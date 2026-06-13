//! MCP catalog + connect. The catalog is a curated list of MCP servers shown
//! with logos in the UI. Because TakoIA agents run on `claude -p`, "connecting"
//! a server registers it with Claude Code (`claude mcp add`) so the agent can
//! actually use its tools.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;
use uuid::Uuid;

/// The curated catalog, embedded at build time.
const CATALOG_JSON: &str = include_str!("../../assets/mcp_catalog.json");

/// `GET /api/mcp/catalog` — the curated MCP server catalog.
pub async fn catalog() -> AppResult<Json<Value>> {
    let parsed: Value = serde_json::from_str(CATALOG_JSON)
        .map_err(|e| AppError::Other(anyhow::anyhow!("invalid catalog: {e}")))?;
    Ok(Json(json!({ "servers": parsed })))
}

/// `GET /api/mcp/installed` — MCP servers currently registered with Claude Code.
pub async fn installed() -> AppResult<Json<Value>> {
    let output = Command::new("claude").arg("mcp").arg("list").output().await;
    let servers = match output {
        Ok(o) if o.status.success() => parse_mcp_list(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    };
    Ok(Json(json!({ "installed": servers })))
}

/// Parse `claude mcp list` lines like `name: <cmd or url> - ✔ Connected`.
fn parse_mcp_list(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, rest) = line.split_once(": ")?;
            let connected = rest.contains('\u{2714}'); // ✔
            Some(json!({ "name": name.trim(), "connected": connected }))
        })
        .collect()
}

#[derive(Deserialize)]
pub struct ConnectMcp {
    /// Catalog server id.
    pub id: String,
    /// Optional env vars (e.g. API keys) as KEY=VALUE pairs.
    #[serde(default)]
    pub env: Vec<String>,
}

/// `POST /api/mcp/connect` — register a catalog server with Claude Code and
/// record it as a connector. Best-effort: reports whether the CLI add succeeded.
pub async fn connect(
    State(state): State<AppState>,
    Json(body): Json<ConnectMcp>,
) -> AppResult<Json<Value>> {
    let catalog: Vec<Value> = serde_json::from_str(CATALOG_JSON)
        .map_err(|e| AppError::Other(anyhow::anyhow!("invalid catalog: {e}")))?;
    let entry = catalog
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(body.id.as_str()))
        .ok_or_else(|| AppError::NotFound("unknown MCP server".into()))?;

    let transport = entry.get("transport").and_then(|v| v.as_str()).unwrap_or("stdio");
    let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or(&body.id);

    // Build `claude mcp add` arguments.
    let mut cmd = Command::new("claude");
    cmd.arg("mcp").arg("add").arg("-s").arg("user");
    for kv in &body.env {
        cmd.arg("-e").arg(kv);
    }
    let target: String;
    if transport == "http" {
        let url = entry.get("url").and_then(|v| v.as_str()).unwrap_or("");
        target = url.to_string();
        cmd.arg("--transport").arg("http").arg(&body.id).arg(url);
    } else {
        let command = entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
        target = command.to_string();
        cmd.arg(&body.id).arg("--");
        for part in command.split_whitespace() {
            cmd.arg(part);
        }
    }

    let result = cmd.output().await;
    let cli_ok = matches!(&result, Ok(o) if o.status.success());
    let cli_msg = match &result {
        Ok(o) if o.status.success() => "registered with Claude Code".to_string(),
        Ok(o) => String::from_utf8_lossy(&o.stderr).chars().take(300).collect(),
        Err(e) => format!("claude CLI unavailable: {e}"),
    };

    // Record as a connector regardless, so it shows as connected in TakoIA.
    sqlx::query(
        r#"INSERT INTO connectors (id, account_id, kind, name, base_url, model, meta)
           VALUES (?, ?, 'mcp', ?, ?, '', ?)
           ON CONFLICT(account_id, kind, name) DO UPDATE SET
             base_url = excluded.base_url, meta = excluded.meta,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&body.id)
    .bind(&target)
    .bind(json!({ "transport": transport, "name": name }).to_string())
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "ok": true, "cli_registered": cli_ok, "message": cli_msg })))
}
