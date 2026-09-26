//! The single `claude -p` launcher, plus the `LlmProvider` built on it.
//!
//! Every place that shells out to Claude Code (agent steps, one-shot
//! generations, video analysis) goes through [`run`], so the security envelope
//! is defined once: sandbox from Settings, cleared environment, an explicit
//! tool set, a hard timeout, and no interactive permission surface.

use super::{Completion, CompletionRequest, LlmProvider, Role, TokenUsage};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Sentinel `base_url` value that selects this transport in the registry.
pub const TRANSPORT_SENTINEL: &str = "claude-cli";

/// Hard wall-clock limit for one agent-step invocation. A hung CLI must not pin
/// a worker forever; on timeout the child is killed (kill_on_drop).
pub const STEP_TIMEOUT: Duration = Duration::from_secs(300);

/// Built-in Claude Code tools the subprocess may use. `--tools` is the real
/// boundary: it removes every other tool from the session. The permission mode
/// only decides whether the remaining tools would prompt, and `-p` cannot
/// prompt, so the listed tools are pre-approved instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSet {
    /// Pure text generation: no tools at all (`--tools ""`).
    None,
    /// Read files under the working directory only.
    ReadOnly,
    /// Live research: web search, web fetch, and reading local files.
    Research,
}

impl ToolSet {
    fn cli_value(self) -> &'static str {
        match self {
            ToolSet::None => "",
            ToolSet::ReadOnly => "Read",
            ToolSet::Research => "WebSearch,WebFetch,Read",
        }
    }
}

/// Where and how a `claude` child runs. Built once per provider or per call.
#[derive(Debug, Clone)]
pub struct ClaudeRuntime {
    pub binary: String,
    /// Plan token (`claude setup-token`); exported as CLAUDE_CODE_OAUTH_TOKEN.
    pub token: Option<String>,
    /// Working directory: cwd, HOME and the sandbox's writable root.
    pub workdir: String,
    pub sandbox: crate::sandbox::SandboxConfig,
    /// Drop a steering CLAUDE.md into the workdir so the agent stays on task.
    pub steering: bool,
}

/// One invocation. `prompt` goes to stdin; `system` to `--append-system-prompt`.
#[derive(Debug, Clone)]
pub struct ClaudeRun<'a> {
    pub system: Option<&'a str>,
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub tools: ToolSet,
    pub timeout: Duration,
}

/// Parsed `--output-format json` result.
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub result: String,
    pub model: String,
    pub usage: TokenUsage,
}

/// Run `claude -p` once.
///
/// Fails on: spawn error (binary missing, sandbox refused), timeout (child
/// killed), non-zero exit (stderr truncated to 500 chars), unparsable JSON, or
/// an `is_error` result. Never retries and never falls back — the caller
/// decides what a failure means.
pub async fn run(rt: &ClaudeRuntime, call: ClaudeRun<'_>) -> anyhow::Result<CliOutput> {
    let mut argv: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--strict-mcp-config".into(),
        "--tools".into(),
        call.tools.cli_value().into(),
    ];
    if call.tools != ToolSet::None {
        // The tool set above is the restriction; this only stops the (already
        // whitelisted) tools from hitting a permission prompt that -p cannot show.
        argv.push("--permission-mode".into());
        argv.push("bypassPermissions".into());
    }
    if let Some(model) = call.model.filter(|m| !m.trim().is_empty()) {
        argv.push("--model".into());
        argv.push(model.to_string());
    }
    if let Some(system) = call.system.filter(|s| !s.trim().is_empty()) {
        argv.push("--append-system-prompt".into());
        argv.push(system.to_string());
    }

    prepare_workdir(&rt.workdir, rt.steering)
        .await
        .with_context(|| format!("failed to prepare workdir {}", rt.workdir))?;

    let mut cmd = crate::sandbox::build_command(
        &rt.sandbox,
        &rt.workdir,
        &rt.binary,
        &argv,
        rt.token.as_deref(),
    );
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `{}`", rt.binary))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(call.prompt.trim().as_bytes())
            .await
            .context("failed to write prompt to claude stdin")?;
        drop(stdin);
    }

    let output = match tokio::time::timeout(call.timeout, child.wait_with_output()).await {
        Ok(res) => res.context("failed to run claude -p")?,
        Err(_) => {
            return Err(anyhow!(
                "claude -p exceeded {}s timeout",
                call.timeout.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "claude -p exited with {}: {}",
            output.status,
            stderr.chars().take(500).collect::<String>()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: CliResult =
        serde_json::from_str(stdout.trim()).context("failed to parse claude -p JSON output")?;
    if parsed.is_error {
        return Err(anyhow!("claude -p reported an error: {}", parsed.result));
    }

    let model = parsed
        .model_usage
        .as_ref()
        .and_then(|m| m.keys().next().cloned())
        .or_else(|| call.model.map(String::from))
        .unwrap_or_else(|| "claude".to_string());

    Ok(CliOutput {
        result: parsed.result,
        model,
        usage: TokenUsage {
            prompt_tokens: parsed
                .usage
                .input_tokens
                .saturating_add(parsed.usage.cache_creation_input_tokens)
                .saturating_add(parsed.usage.cache_read_input_tokens),
            completion_tokens: parsed.usage.output_tokens,
        },
    })
}

/// Steering file dropped into every agent workdir. Constant, so it is only
/// written when missing — but checked per workdir (there is one per agent),
/// not once per process.
const STEERING_CLAUDE_MD: &str = "# Task agent\n\n\
    You are an autonomous task agent. Focus only on the task in the system \
    prompt and user message. Use whatever tools are available to you \
    (including web search and web fetch) to gather real information and \
    complete the task. Do not reference any codebase or repository. \
    Produce the requested deliverable directly.\n";

async fn prepare_workdir(workdir: &str, steering: bool) -> std::io::Result<()> {
    // The private tmp is what TMPDIR points at (see sandbox::child_env).
    tokio::fs::create_dir_all(format!("{workdir}/tmp")).await?;
    if steering {
        let path = std::path::Path::new(workdir).join("CLAUDE.md");
        if tokio::fs::metadata(&path).await.is_err() {
            tokio::fs::write(&path, STEERING_CLAUDE_MD).await?;
        }
    }
    Ok(())
}

/// A provider that shells out to `claude -p` for agent steps.
pub struct ClaudeCliProvider {
    name: String,
    runtime: ClaudeRuntime,
    default_model: Option<String>,
}

impl ClaudeCliProvider {
    pub fn new(
        name: impl Into<String>,
        default_model: Option<String>,
        token: Option<String>,
        workdir: String,
        sandbox: crate::sandbox::SandboxConfig,
    ) -> Self {
        Self {
            name: name.into(),
            runtime: ClaudeRuntime {
                binary: "claude".to_string(),
                token: token.filter(|t| !t.is_empty()),
                workdir,
                sandbox,
                steering: true,
            },
            default_model: default_model.filter(|m| !m.is_empty()),
        }
    }
}

#[async_trait]
impl LlmProvider for ClaudeCliProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_web_search(&self) -> bool {
        true
    }

    async fn complete(&self, req: CompletionRequest) -> anyhow::Result<Completion> {
        let mut system = String::new();
        let mut prompt = String::new();
        for m in &req.messages {
            match m.role {
                Role::System => {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&m.content);
                }
                Role::User => {
                    prompt.push_str(&m.content);
                    prompt.push_str("\n\n");
                }
                Role::Assistant => {
                    prompt.push_str("(previous assistant note) ");
                    prompt.push_str(&m.content);
                    prompt.push_str("\n\n");
                }
            }
        }
        let model = req.model.as_deref().or(self.default_model.as_deref());
        let tools = if req.enable_web_search {
            ToolSet::Research
        } else {
            ToolSet::ReadOnly
        };
        let out = run(
            &self.runtime,
            ClaudeRun {
                system: Some(&system),
                prompt: &prompt,
                model,
                tools,
                timeout: STEP_TIMEOUT,
            },
        )
        .await?;
        Ok(Completion {
            content: out.result,
            model: out.model,
            usage: out.usage,
        })
    }
}

#[derive(Deserialize)]
struct CliResult {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: String,
    #[serde(default)]
    usage: CliUsage,
    #[serde(rename = "modelUsage")]
    model_usage: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize, Default)]
struct CliUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_sets_map_to_explicit_cli_values() {
        assert_eq!(ToolSet::None.cli_value(), "");
        assert_eq!(ToolSet::ReadOnly.cli_value(), "Read");
        assert_eq!(ToolSet::Research.cli_value(), "WebSearch,WebFetch,Read");
    }

    #[test]
    fn cli_usage_sums_cache_tokens_without_overflow() {
        let parsed: CliResult = serde_json::from_str(
            r#"{"result":"ok","usage":{"input_tokens":4294967295,"output_tokens":3,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}}"#,
        )
        .unwrap();
        let prompt = parsed
            .usage
            .input_tokens
            .saturating_add(parsed.usage.cache_creation_input_tokens)
            .saturating_add(parsed.usage.cache_read_input_tokens);
        assert_eq!(prompt, u32::MAX);
        assert!(!parsed.is_error);
    }

    #[tokio::test]
    async fn steering_file_is_written_per_workdir_and_only_once() {
        let base = std::env::temp_dir().join(format!("takoia-steer-{}", uuid::Uuid::new_v4()));
        let a = base.join("agent-a");
        let b = base.join("agent-b");
        prepare_workdir(a.to_str().unwrap(), true).await.unwrap();
        prepare_workdir(b.to_str().unwrap(), true).await.unwrap();
        assert!(a.join("CLAUDE.md").exists(), "first workdir gets steering");
        assert!(b.join("CLAUDE.md").exists(), "second workdir gets it too");
        tokio::fs::write(a.join("CLAUDE.md"), "custom")
            .await
            .unwrap();
        prepare_workdir(a.to_str().unwrap(), true).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(a.join("CLAUDE.md"))
                .await
                .unwrap(),
            "custom",
            "an existing file is never overwritten"
        );
        let _ = tokio::fs::remove_dir_all(base).await;
    }
}
