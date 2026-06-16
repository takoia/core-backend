//! One-shot `claude -p` helpers for internal generation (scaffolding, persona
//! evolution, inner-life reflection). These run the SAME execution sandbox as
//! agent runs (default Landlock), so an internal call can never write outside
//! its confined workdir either. Centralised here to avoid duplicating the spawn.

use crate::state::AppState;
use serde_json::Value;
use std::time::Duration;

const TIMEOUT_SECS: u64 = 180;

/// Run a one-shot `claude -p` and return the raw result text, confined by the
/// active sandbox. Returns `None` on spawn/timeout/non-zero exit.
pub async fn generate_text(state: &AppState, prompt: &str) -> Option<String> {
    let argv: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];
    // Dedicated confined workdir for internal generations.
    let workdir = format!("{}/_internal", state.config.agent_workdir);
    let _ = std::fs::create_dir_all(&workdir);
    let cfg = crate::sandbox::active(&state.db).await;
    let mut cmd = crate::sandbox::build_command(
        &cfg,
        &workdir,
        "claude",
        &argv,
        state.config.claude_max_token.as_deref(),
    );
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(prompt.as_bytes()).await;
        drop(stdin);
    }
    let output = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).ok()?;
    Some(
        parsed
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    )
}

/// Run a one-shot `claude -p` and parse a JSON object out of the result
/// (tolerating surrounding prose / code fences), confined by the active sandbox.
pub async fn generate_json(state: &AppState, prompt: &str) -> Option<Value> {
    let text = generate_text(state, prompt).await?;
    let slice = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => &text,
    };
    serde_json::from_str(slice).ok()
}
