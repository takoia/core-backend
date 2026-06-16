//! Connectors / Settings: LLM providers (and Discord) with encrypted secrets.
//! Secrets are never returned in clear — only a masked hint.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct ConnectorRow {
    id: String,
    kind: String,
    name: String,
    base_url: String,
    model: String,
    encrypted_secret: Option<Vec<u8>>,
    is_default: i64,
}

#[derive(Serialize)]
struct ConnectorView {
    id: String,
    kind: String,
    name: String,
    base_url: String,
    model: String,
    has_secret: bool,
    secret_hint: String,
    is_default: bool,
}

/// `GET /api/connectors` — list providers with masked secrets.
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query_as::<_, ConnectorRow>(
        r#"SELECT id, kind, name, base_url, model, encrypted_secret, is_default
           FROM connectors WHERE account_id = ? ORDER BY kind, name"#,
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_all(&state.db)
    .await?;

    let views: Vec<ConnectorView> = rows
        .into_iter()
        .map(|r| {
            let has_secret = r.encrypted_secret.as_ref().map(|b| !b.is_empty()).unwrap_or(false);
            ConnectorView {
                id: r.id,
                kind: r.kind,
                name: r.name,
                base_url: r.base_url,
                model: r.model,
                has_secret,
                secret_hint: if has_secret { "••••••••".into() } else { String::new() },
                is_default: r.is_default != 0,
            }
        })
        .collect();
    Ok(Json(json!({ "connectors": views })))
}

#[derive(Deserialize)]
pub struct UpsertConnector {
    #[serde(default = "default_kind")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    /// New secret value; if omitted/empty the existing secret is kept.
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

fn default_kind() -> String {
    "llm".into()
}

/// `POST /api/connectors` — create or update a provider (encrypts the secret).
pub async fn upsert(
    State(state): State<AppState>,
    Json(body): Json<UpsertConnector>,
) -> AppResult<Json<Value>> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    // SSRF guard: the server later calls this base_url with the stored key, so
    // reject internal/metadata targets. The "claude-cli" sentinel is not a URL.
    let base = body.base_url.trim();
    if !base.is_empty() && base != crate::llm::CLAUDE_CLI_SENTINEL {
        crate::net::validate_outbound_url(base)
            .await
            .map_err(|e| AppError::BadRequest(format!("base_url rejected: {e}")))?;
    }
    // Route the secret through the active backend (local cipher, or an external
    // vault). Scope is stable across upserts (the conflict key is kind + name).
    let encrypted = match body.secret.as_ref().filter(|s| !s.trim().is_empty()) {
        Some(secret) => {
            let scope = format!("{}-{}", body.kind, body.name.trim());
            let sm = crate::secrets::SecretManager::new(&state.cipher, &state.db);
            Some(sm.store_secret(&scope, secret).await.map_err(AppError::Other)?)
        }
        None => None,
    };

    // Reset other defaults if this one is becoming default.
    if body.is_default {
        sqlx::query("UPDATE connectors SET is_default = 0 WHERE account_id = ? AND kind = ?")
            .bind(DEFAULT_ACCOUNT_ID)
            .bind(&body.kind)
            .execute(&state.db)
            .await?;
    }

    // Upsert on (account, kind, name). Keep the existing secret if none given.
    sqlx::query(
        r#"INSERT INTO connectors (id, account_id, kind, name, base_url, model, encrypted_secret, is_default)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(account_id, kind, name) DO UPDATE SET
             base_url = excluded.base_url,
             model = excluded.model,
             encrypted_secret = COALESCE(excluded.encrypted_secret, connectors.encrypted_secret),
             is_default = excluded.is_default,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&body.kind)
    .bind(&body.name)
    .bind(&body.base_url)
    .bind(&body.model)
    .bind(encrypted)
    .bind(body.is_default as i64)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/connectors/:id` — remove a provider.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM connectors WHERE id = ? AND account_id = ?")
        .bind(&id)
        .bind(DEFAULT_ACCOUNT_ID)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
