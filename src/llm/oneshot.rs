//! One-shot text/JSON generations (persona evolution, scaffolding, reflection,
//! external agent import). They embed untrusted text — memories, uploaded
//! definitions, user descriptions — so they run with NO tools at all.

use super::claude_cli::{self, ClaudeRun, ClaudeRuntime, ToolSet};
use crate::state::AppState;
use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(180);

/// Run a tool-less `claude -p` and return the raw result text, confined by the
/// active sandbox. Returns `None` on any failure (spawn, timeout, non-zero
/// exit, bad JSON); the failure is logged so it is not silent.
pub async fn generate_text(state: &AppState, prompt: &str) -> Option<String> {
    let rt = ClaudeRuntime {
        binary: "claude".to_string(),
        token: state.config.claude_max_token.clone(),
        workdir: format!("{}/_internal", state.config.agent_workdir),
        sandbox: crate::sandbox::active(&state.db)
            .await
            .with_passthrough(&state.config.agent_env_passthrough),
        steering: false,
    };
    match claude_cli::run(
        &rt,
        ClaudeRun {
            system: None,
            prompt,
            model: None,
            tools: ToolSet::None,
            timeout: TIMEOUT,
        },
    )
    .await
    {
        Ok(out) => Some(out.result),
        Err(e) => {
            tracing::warn!(error = %e, "one-shot claude generation failed");
            None
        }
    }
}

/// Run a one-shot generation and parse a JSON object out of the result
/// (tolerating surrounding prose / code fences).
pub async fn generate_json(state: &AppState, prompt: &str) -> Option<Value> {
    let text = generate_text(state, prompt).await?;
    extract_json_object(&text)
}

/// Pull the outermost `{...}` out of free text and parse it.
pub(crate) fn extract_json_object(text: &str) -> Option<Value> {
    let slice = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => text,
    };
    serde_json::from_str(slice).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_object_from_fenced_prose() {
        let v =
            extract_json_object("Sure!\n```json\n{\"a\": 1, \"b\": \"x\"}\n```\nDone.").unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x");
    }

    #[test]
    fn rejects_text_without_object() {
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("} {").is_none());
    }
}
