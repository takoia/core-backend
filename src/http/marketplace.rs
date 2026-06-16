//! Marketplace: publish specialized expert agents and resell them as a hosted
//! API, billed per outgoing token. The agent and its ICM memory never leave the
//! platform — consumers call an API; the memory stays read-only and integrated.

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
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Json(body): Json<NewKey>,
) -> AppResult<Json<Value>> {
    let secret = format!("sk_takoia_{}", Uuid::new_v4().simple());
    let prefix = secret.chars().take(16).collect::<String>();
    sqlx::query(
        r#"INSERT INTO api_keys (id, account_id, name, key_hash, key_prefix)
           VALUES (?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&me.account_id)
    .bind(&body.name)
    .bind(hash_key(&secret))
    .bind(&prefix)
    .execute(&state.db)
    .await?;
    // The plaintext is returned only here, never stored.
    Ok(Json(json!({ "key": secret, "prefix": prefix })))
}

/// `GET /api/keys` — list the account's API keys (prefixes only).
pub async fn list_keys(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
) -> AppResult<Json<Value>> {
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
    .bind(&me.account_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "keys": rows })))
}

/// `DELETE /api/keys/:id` — revoke an API key (scoped to the caller's account).
pub async fn revoke_key(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = ? AND account_id = ?")
        .bind(&id)
        .bind(&me.account_id)
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

/// `GET /api/marketplace/usage` — per-request usage detail: the most recent
/// metered invocations with their token counts and the price billed for each.
pub async fn usage(State(state): State<AppState>) -> AppResult<Json<Value>> {
    #[derive(Serialize, sqlx::FromRow)]
    struct UsageRow {
        id: String,
        agent_id: String,
        agent_name: String,
        prompt_tokens: i64,
        completion_tokens: i64,
        billed_usd: f64,
        publisher_usd: f64,
        created_at: String,
    }
    let rows = sqlx::query_as::<_, UsageRow>(
        r#"SELECT u.id, u.agent_id,
                  COALESCE(a.name, u.agent_id) AS agent_name,
                  u.prompt_tokens, u.completion_tokens, u.billed_usd, u.publisher_usd, u.created_at
           FROM marketplace_usage u
           LEFT JOIN agents a ON a.id = u.agent_id
           ORDER BY u.created_at DESC
           LIMIT 100"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "usage": rows })))
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

    let r = run_and_bill(&state, &id, &body.input, &consumer).await?;
    Ok(Json(json!({
        "agent": r.name,
        "output": r.output_text,
        "usage": { "prompt_tokens": r.prompt_tokens, "completion_tokens": r.completion_tokens },
        "cost_usd": r.billed_usd,
        "publisher_earned_usd": r.publisher_usd,
    })))
}

/// Outcome of running a published agent once, with metered token usage and the
/// amounts already recorded in `marketplace_usage`.
struct InvokeResult {
    name: String,
    output_text: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    billed_usd: f64,
    publisher_usd: f64,
}

/// Run a published agent synchronously (read-only memory), meter its outgoing
/// tokens, bill the consumer, and credit the publisher's share. Shared by the
/// native invoke API and the OpenAI-compatible endpoint.
async fn run_and_bill(
    state: &AppState,
    id: &str,
    input: &str,
    consumer: &str,
) -> AppResult<InvokeResult> {
    if input.trim().is_empty() {
        return Err(AppError::BadRequest("input is required".into()));
    }
    let agent: Option<(String, String, f64, f64, String)> = sqlx::query_as(
        "SELECT name, visibility, price_per_1k_output_tokens, revenue_share, account_id
         FROM agents WHERE id = ?",
    )
    .bind(id)
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
    .bind(id)
    .bind("api invoke")
    .bind(input)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO jobs (id, objective_id, agent_id, status, synchronous) VALUES (?, ?, ?, 'running', 1)")
        .bind(&job_id)
        .bind(&objective_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let claimed = ClaimedJob {
        id: job_id.clone(),
        objective_id,
        agent_id: id.to_string(),
    };
    // read_only_memory = true: never write to the publisher's curated memory.
    match crate::agent::engine::run_job(state, &claimed, true).await {
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

    // Token usage for this job → metered billing.
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
    .bind(id)
    .bind(&publisher)
    .bind(consumer)
    .bind(&job_id)
    .bind(pt)
    .bind(ct)
    .bind(billed)
    .bind(publisher_usd)
    .execute(&state.db)
    .await?;

    Ok(InvokeResult {
        name,
        output_text,
        prompt_tokens: pt,
        completion_tokens: ct,
        billed_usd: billed,
        publisher_usd,
    })
}

// ── OpenAI-compatible API ──────────────────────────────────────────────────
// Any OpenAI SDK / tool can call a published agent by pointing its base_url at
// `<host>/api/v1` and using the agent id as the `model`. Authenticated with a
// marketplace key (`sk_takoia_...`); usage is metered and billed identically to
// the native invoke API.

#[derive(Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Deserialize)]
pub struct ChatCompletionRequest {
    /// The published agent id to run (OpenAI's `model` field).
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
}

/// `POST /api/v1/chat/completions` — OpenAI Chat Completions-compatible entry
/// point. The `model` is a published agent id; the conversation is flattened
/// into the agent's objective and the restitution is returned as the assistant
/// message.
pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChatCompletionRequest>,
) -> AppResult<Json<Value>> {
    let consumer = auth_key(&state, &headers).await?;
    if body.stream {
        return Err(AppError::BadRequest(
            "streaming is not supported yet; set stream=false".into(),
        ));
    }
    // Flatten the conversation into the agent's objective prompt. The last user
    // message is the request; earlier messages are kept as context.
    let input = body
        .messages
        .iter()
        .filter(|m| !m.content.trim().is_empty())
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let r = run_and_bill(&state, &body.model, &input, &consumer).await?;

    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(Json(json!({
        "id": format!("chatcmpl-{}", Uuid::new_v4().simple()),
        "object": "chat.completion",
        "created": created,
        "model": body.model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": r.output_text },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": r.prompt_tokens,
            "completion_tokens": r.completion_tokens,
            "total_tokens": r.prompt_tokens + r.completion_tokens,
        },
    })))
}

/// `GET /api/v1/models` — OpenAI-compatible model list: every published agent is
/// exposed as a callable "model".
pub async fn list_models(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, name FROM agents WHERE visibility = 'public' ORDER BY name")
            .fetch_all(&state.db)
            .await?;
    let data: Vec<Value> = rows
        .into_iter()
        .map(|(id, name)| {
            json!({
                "id": id,
                "object": "model",
                "owned_by": "takoia",
                "name": name,
            })
        })
        .collect();
    Ok(Json(json!({ "object": "list", "data": data })))
}
