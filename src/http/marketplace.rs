//! Marketplace: publish specialized expert agents and resell them as a hosted
//! API, billed per outgoing token. The agent and its ICM memory never leave the
//! platform — consumers call an API; the memory stays read-only and integrated.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::queue::ClaimedJob;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// `GET /api/marketplace` — list published (public) expert agents with pricing.
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    #[derive(Serialize, sqlx::FromRow)]
    struct PublicAgent {
        id: String,
        name: String,
        description: String,
        expertise_domain: String,
        icon: String,
        price_per_1k_output_tokens: f64,
        revenue_share: f64,
        runs_count: i64,
        published_at: Option<String>,
    }
    let rows = sqlx::query_as::<_, PublicAgent>(
        r#"SELECT id, name, description, expertise_domain, icon,
                  price_per_1k_output_tokens, revenue_share, runs_count, published_at
           FROM agents WHERE visibility = 'public'
           ORDER BY runs_count DESC, published_at DESC"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "agents": rows })))
}

// ── Consumer API keys ──────────────────────────────────────────────────────

fn hash_key(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    format!("{:x}", h.finalize())
}

#[derive(Deserialize)]
pub struct NewKey {
    #[serde(default)]
    pub name: String,
}

/// `POST /api/keys` — create a consumer API key (plaintext shown once).
pub async fn create_key(
    State(state): State<AppState>,
    Json(body): Json<NewKey>,
) -> AppResult<Json<Value>> {
    let secret = format!("sk_takoia_{}", Uuid::new_v4().simple());
    let prefix = secret.chars().take(16).collect::<String>();
    sqlx::query(
        r#"INSERT INTO api_keys (id, account_id, name, key_hash, key_prefix)
           VALUES (?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&body.name)
    .bind(hash_key(&secret))
    .bind(&prefix)
    .execute(&state.db)
    .await?;
    // The plaintext is returned only here, never stored.
    Ok(Json(json!({ "key": secret, "prefix": prefix })))
}

/// `GET /api/keys` — list the account's API keys (prefixes only).
pub async fn list_keys(State(state): State<AppState>) -> AppResult<Json<Value>> {
    #[derive(Serialize, sqlx::FromRow)]
    struct KeyRow {
        id: String,
        name: String,
        key_prefix: String,
        revoked: i64,
        last_used_at: Option<String>,
        created_at: String,
    }
    let rows = sqlx::query_as::<_, KeyRow>(
        "SELECT id, name, key_prefix, revoked, last_used_at, created_at
         FROM api_keys WHERE account_id = ? ORDER BY created_at DESC",
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "keys": rows })))
}

/// `DELETE /api/keys/:id` — revoke an API key.
pub async fn revoke_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/marketplace/earnings` — publisher revenue + consumer spend summary.
pub async fn earnings(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let row: (i64, i64, f64, f64) = sqlx::query_as(
        r#"SELECT COUNT(*),
                  COALESCE(SUM(completion_tokens), 0),
                  COALESCE(SUM(billed_usd), 0.0),
                  COALESCE(SUM(publisher_usd), 0.0)
           FROM marketplace_usage"#,
    )
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({
        "invokes": row.0,
        "output_tokens": row.1,
        "billed_usd": row.2,
        "publisher_usd": row.3,
    })))
}

// ── Public hosted-agent API (token-billed) ─────────────────────────────────

#[derive(Deserialize)]
pub struct InvokeInput {
    pub input: String,
}

/// Authenticate a `Bearer sk_...` key, returning the consumer account id.
async fn auth_key(state: &AppState, headers: &HeaderMap) -> AppResult<String> {
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let key = raw.strip_prefix("Bearer ").unwrap_or(raw).trim();
    if key.is_empty() {
        return Err(AppError::Unauthorized("missing API key".into()));
    }
    let key_hash = hash_key(key);
    let row: Option<(String,)> =
        sqlx::query_as("SELECT account_id FROM api_keys WHERE key_hash = ? AND revoked = 0")
            .bind(&key_hash)
            .fetch_optional(&state.db)
            .await?;
    let account = row
        .map(|(a,)| a)
        .ok_or_else(|| AppError::Unauthorized("invalid API key".into()))?;
    let _ = sqlx::query(
        "UPDATE api_keys SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE key_hash = ?",
    )
    .bind(&key_hash)
    .execute(&state.db)
    .await;
    Ok(account)
}

/// `POST /api/v1/agents/:id/invoke` — call a published agent over HTTP. Runs the
/// agent synchronously with read-only memory, meters the outgoing tokens, bills
/// the consumer, and credits the publisher's share.
pub async fn invoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InvokeInput>,
) -> AppResult<Json<Value>> {
    let consumer = auth_key(&state, &headers).await?;
    if body.input.trim().is_empty() {
        return Err(AppError::BadRequest("input is required".into()));
    }

    let agent: Option<(String, String, f64, f64, String)> = sqlx::query_as(
        "SELECT name, visibility, price_per_1k_output_tokens, revenue_share, account_id
         FROM agents WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let (name, visibility, price_per_1k, rev_share, publisher) =
        agent.ok_or_else(|| AppError::NotFound("agent not found".into()))?;
    if visibility != "public" {
        return Err(AppError::BadRequest("agent is not published".into()));
    }

    // Create the job already 'running' so the background worker skips it; we run
    // it inline for a synchronous response.
    let objective_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO objectives (id, account_id, agent_id, title, prompt) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&objective_id)
    .bind(&publisher)
    .bind(&id)
    .bind("api invoke")
    .bind(&body.input)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO jobs (id, objective_id, agent_id, status, synchronous) VALUES (?, ?, ?, 'running', 1)")
        .bind(&job_id)
        .bind(&objective_id)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let claimed = ClaimedJob {
        id: job_id.clone(),
        objective_id,
        agent_id: id.clone(),
    };
    // read_only_memory = true: never write to the publisher's curated memory.
    match crate::agent::engine::run_job(&state, &claimed, true).await {
        Ok(crate::agent::engine::RunOutcome::AwaitingApproval) => {
            crate::queue::mark_failed(
                &state.db,
                &job_id,
                "agent requires interactive approval; not available via the synchronous invoke API",
            )
            .await
            .ok();
            return Err(AppError::BadRequest(
                "This agent requires human approval before acting and cannot be invoked via the synchronous API".into(),
            ));
        }
        Ok(_) => {}
        Err(e) => {
            return Err(AppError::Other(anyhow::anyhow!("agent run failed: {e}")));
        }
    }

    // The deliverable is the restitution step output.
    let output: Option<(String,)> = sqlx::query_as(
        "SELECT output FROM steps WHERE job_id = ? AND step_type = 'restitution'
         ORDER BY position DESC LIMIT 1",
    )
    .bind(&job_id)
    .fetch_optional(&state.db)
    .await?;
    let output_text = output
        .map(|(o,)| {
            serde_json::from_str::<Value>(&o)
                .ok()
                .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
                .unwrap_or(o)
        })
        .unwrap_or_default();

    // Token usage for this job → mock billing.
    let (pt, ct): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0)
         FROM token_usage WHERE job_id = ?",
    )
    .bind(&job_id)
    .fetch_one(&state.db)
    .await?;
    let billed = (ct as f64 / 1000.0) * price_per_1k;
    let publisher_usd = billed * rev_share;

    sqlx::query(
        r#"INSERT INTO marketplace_usage
             (id, agent_id, publisher_account, consumer_account, job_id,
              prompt_tokens, completion_tokens, billed_usd, publisher_usd)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&id)
    .bind(&publisher)
    .bind(&consumer)
    .bind(&job_id)
    .bind(pt)
    .bind(ct)
    .bind(billed)
    .bind(publisher_usd)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({
        "agent": name,
        "output": output_text,
        "usage": { "prompt_tokens": pt, "completion_tokens": ct },
        "cost_usd": billed,
        "publisher_earned_usd": publisher_usd,
    })))
}
