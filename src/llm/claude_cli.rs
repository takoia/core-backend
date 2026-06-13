//! Claude provider backed by the Claude Code CLI in headless mode (`claude -p`).
//!
//! This consumes the user's Claude **plan** (forfait) via a long-lived token
//! generated with `claude setup-token`, instead of per-token API billing. The
//! token, when provided, is passed to the subprocess as `CLAUDE_CODE_OAUTH_TOKEN`.

use super::{Completion, CompletionRequest, LlmProvider, Role, TokenUsage};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Sentinel `base_url` value that selects this transport in the registry.
pub const TRANSPORT_SENTINEL: &str = "claude-cli";

/// A provider that shells out to `claude -p --output-format json`.
pub struct ClaudeCliProvider {
    name: String,
    binary: String,
    default_model: Option<String>,
    /// Plan token (`claude setup-token`); set as CLAUDE_CODE_OAUTH_TOKEN.
    token: Option<String>,
}

impl ClaudeCliProvider {
    pub fn new(
        name: impl Into<String>,
        default_model: Option<String>,
        token: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            binary: "claude".to_string(),
            default_model: default_model.filter(|m| !m.is_empty()),
            token: token.filter(|t| !t.is_empty()),
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

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-p")
            .arg("--output-format")
            .arg("json")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(model) = &model {
            cmd.arg("--model").arg(model);
        }
        if !system.trim().is_empty() {
            cmd.arg("--append-system-prompt").arg(&system);
        }
        if let Some(token) = &self.token {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }

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

        let output = child
            .wait_with_output()
            .await
            .context("failed to run claude -p")?;

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
                prompt_tokens: parsed.usage.input_tokens,
                completion_tokens: parsed.usage.output_tokens,
            },
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
}
