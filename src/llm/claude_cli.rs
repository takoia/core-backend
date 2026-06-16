//! Claude provider backed by the Claude Code CLI in headless mode (`claude -p`).
//!
//! This consumes the user's Claude **plan** (subscription) via a long-lived token
//! generated with `claude setup-token`, instead of per-token API billing. The
//! token, when provided, is passed to the subprocess as `CLAUDE_CODE_OAUTH_TOKEN`.

use super::{Completion, CompletionRequest, LlmProvider, Role, TokenUsage};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Sentinel `base_url` value that selects this transport in the registry.
pub const TRANSPORT_SENTINEL: &str = "claude-cli";

/// Hard wall-clock limit for a single `claude -p` invocation. A hung CLI must
/// not pin a worker forever; on timeout the child is killed (kill_on_drop) and
/// the call returns an error so the engine can fall back to its canned provider.
const CLAUDE_CLI_TIMEOUT_SECS: u64 = 300;

/// A provider that shells out to `claude -p --output-format json`.
pub struct ClaudeCliProvider {
    name: String,
    binary: String,
    default_model: Option<String>,
    /// Plan token (`claude setup-token`); set as CLAUDE_CODE_OAUTH_TOKEN.
    token: Option<String>,
    /// Isolated working directory so the agent does not inherit the host
    /// project's CLAUDE.md / project ICM context.
    workdir: Option<String>,
    /// Execution sandbox applied to the subprocess (Landlock / container / …).
    sandbox: crate::sandbox::SandboxConfig,
}

impl ClaudeCliProvider {
    pub fn new(
        name: impl Into<String>,
        default_model: Option<String>,
        token: Option<String>,
        workdir: Option<String>,
        sandbox: crate::sandbox::SandboxConfig,
    ) -> Self {
        Self {
            name: name.into(),
            binary: "claude".to_string(),
            default_model: default_model.filter(|m| !m.is_empty()),
            token: token.filter(|t| !t.is_empty()),
            workdir: workdir.filter(|w| !w.is_empty()),
            sandbox,
        }
    }
}

#[async_trait]
impl LlmProvider for ClaudeCliProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: CompletionRequest) -> anyhow::Result<Completion> {
        // System messages become the appended system prompt; user/assistant
        // messages are flattened into the stdin prompt.
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

        let model = req.model.clone().or_else(|| self.default_model.clone());

        // Build the `claude` argv. Headless `-p` can't answer permission prompts,
        // so every step runs with bypassPermissions; the OS-level sandbox (chosen
        // in Settings) is what actually confines the subprocess.
        let mut argv: Vec<String> = vec![
            "-p".into(),
            "--output-format".into(),
            "json".into(),
            // Do not inherit the host's configured MCP servers.
            "--strict-mcp-config".into(),
        ];
        if let Some(model) = &model {
            argv.push("--model".into());
            argv.push(model.clone());
        }
        if !system.trim().is_empty() {
            argv.push("--append-system-prompt".into());
            argv.push(system.clone());
        }
        argv.push("--permission-mode".into());
        argv.push("bypassPermissions".into());
        argv.push("--allowedTools".into());
        argv.push("WebSearch,WebFetch,Read".into());

        // Run in an isolated, per-agent workdir, confined by the active sandbox.
        let mut cmd = match &self.workdir {
            Some(workdir) => {
                ensure_isolated_workdir(workdir);
                crate::sandbox::build_command(
                    &self.sandbox,
                    workdir,
                    &self.binary,
                    &argv,
                    self.token.as_deref(),
                )
            }
            None => {
                let mut c = Command::new(&self.binary);
                c.args(&argv);
                if let Some(token) = &self.token {
                    c.env("CLAUDE_CODE_OAUTH_TOKEN", token);
                }
                c
            }
        };
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Guarantee the subprocess dies if we drop the child on timeout.
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn `{}`", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.trim().as_bytes())
                .await
                .context("failed to write prompt to claude stdin")?;
            drop(stdin);
        }

        // Race the subprocess against a hard timeout. On elapse, the child is
        // dropped here and `kill_on_drop(true)` terminates the hung process.
        let output = match tokio::time::timeout(
            Duration::from_secs(CLAUDE_CLI_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(res) => res.context("failed to run claude -p")?,
            Err(_) => {
                return Err(anyhow!(
                    "claude -p exceeded {CLAUDE_CLI_TIMEOUT_SECS}s timeout"
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
        let parsed: CliResult = serde_json::from_str(stdout.trim())
            .context("failed to parse claude -p JSON output")?;

        if parsed.is_error {
            return Err(anyhow!("claude -p reported an error: {}", parsed.result));
        }

        let used_model = parsed
            .model_usage
            .as_ref()
            .and_then(|m| m.keys().next().cloned())
            .or(model)
            .unwrap_or_else(|| "claude".to_string());

        Ok(Completion {
            content: parsed.result,
            model: used_model,
            usage: TokenUsage {
                // Cached prompt tokens (creation + read) are billed/counted as
                // input but reported in separate fields; include them all.
                prompt_tokens: parsed
                    .usage
                    .input_tokens
                    .saturating_add(parsed.usage.cache_creation_input_tokens)
                    .saturating_add(parsed.usage.cache_read_input_tokens),
                completion_tokens: parsed.usage.output_tokens,
            },
        })
    }
}

/// Ensure the isolated workdir exists and contains a steering CLAUDE.md that
/// scopes the agent strictly to the task it is given.
///
/// `create_dir_all` is idempotent and cheap, so it runs on every call. The
/// steering CLAUDE.md content is constant, so it is written at most once per
/// process (guarded by a `Once`) to avoid a blocking fs write on every LLM step
/// and a data race when concurrent jobs share the same workdir.
fn ensure_isolated_workdir(workdir: &str) {
    static STEERING_WRITTEN: std::sync::Once = std::sync::Once::new();
    let _ = std::fs::create_dir_all(workdir);
    STEERING_WRITTEN.call_once(|| {
        let claude_md = std::path::Path::new(workdir).join("CLAUDE.md");
        let _ = std::fs::write(
            &claude_md,
            "# Task agent\n\n\
             You are an autonomous task agent. Focus only on the task in the system \
             prompt and user message. Use whatever tools are available to you \
             (including web search and web fetch) to gather real information and \
             complete the task. Do not reference any codebase or repository. \
             Produce the requested deliverable directly.\n",
        );
    });
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
