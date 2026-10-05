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
    /// Requests per minute allowed on this key (default 60; 0 = unlimited).
    #[serde(default)]
    pub rate_limit_per_min: Option<i64>,
}

/// `POST /api/keys` — create a consumer API key (plaintext shown once).
pub async fn create_key(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Json(body): Json<NewKey>,
) -> AppResult<Json<Value>> {
    let secret = format!("sk_takoia_{}", Uuid::new_v4().simple());
    let prefix = secret.chars().take(16).collect::<String>();
    let rate = body.rate_limit_per_min.unwrap_or(60).max(0);
    sqlx::query(
        r#"INSERT INTO api_keys (id, account_id, name, key_hash, key_prefix, rate_limit_per_min)
           VALUES (?, ?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&me.account_id)
    .bind(&body.name)
    .bind(hash_key(&secret))
    .bind(&prefix)
    .bind(rate)
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
pub async fn earnings(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
) -> AppResult<Json<Value>> {
    let row: (i64, i64, f64, f64) = sqlx::query_as(
        r#"SELECT COUNT(*),
                  COALESCE(SUM(completion_tokens), 0),
                  COALESCE(SUM(billed_usd), 0.0),
                  COALESCE(SUM(publisher_usd), 0.0)
           FROM marketplace_usage WHERE publisher_account = ?"#,
    )
    .bind(&me.account_id)
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
pub async fn usage(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
) -> AppResult<Json<Value>> {
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
           WHERE u.publisher_account = ?1 OR u.consumer_account = ?1
           ORDER BY u.created_at DESC
           LIMIT 100"#,
    )
    .bind(&me.account_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "usage": rows })))
}

// ── Public hosted-agent API (token-billed) ─────────────────────────────────

#[derive(Deserialize)]
pub struct InvokeInput {
    pub input: String,
}

/// Authenticate a `Bearer sk_...` key, returning the consumer key record.
async fn auth_key(state: &AppState, headers: &HeaderMap) -> AppResult<crate::billing::ConsumerKey> {
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let key = raw.strip_prefix("Bearer ").unwrap_or(raw).trim();
    if key.is_empty() {
        return Err(AppError::Unauthorized("missing API key".into()));
    }
    let key_hash = hash_key(key);
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT id, account_id, rate_limit_per_min FROM api_keys WHERE key_hash = ? AND revoked = 0",
    )
    .bind(&key_hash)
    .fetch_optional(&state.db)
    .await?;
    let (api_key_id, account_id, rate_limit_per_min) =
        row.ok_or_else(|| AppError::Unauthorized("invalid API key".into()))?;
    let _ = sqlx::query(
        "UPDATE api_keys SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE key_hash = ?",
    )
    .bind(&key_hash)
    .execute(&state.db)
    .await;
    Ok(crate::billing::ConsumerKey {
        account_id,
        api_key_id,
        rate_limit_per_min,
    })
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
        "demo": r.demo,
        "self_invoke": r.self_invoke,
    })))
}

/// Steps that may produce billable output in one invoke: the four loop steps
/// plus one web search. Sizes the credit reservation.
const INVOKE_BILLABLE_STEPS: u32 = 5;

/// Outcome of running a published agent once, with metered token usage and the
/// amounts already recorded in `marketplace_usage`.
struct InvokeResult {
    name: String,
    output_text: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    billed_usd: f64,
    publisher_usd: f64,
    /// Output came (at least partly) from the offline demo provider.
    demo: bool,
    /// The consumer is the publisher's own account.
    self_invoke: bool,
}

/// Run a published agent synchronously (read-only memory), meter its outgoing
/// tokens, bill the consumer, and credit the publisher's share. Shared by the
/// native invoke API and the OpenAI-compatible endpoint.
async fn run_and_bill(
    state: &AppState,
    id: &str,
    input: &str,
    key: &crate::billing::ConsumerKey,
) -> AppResult<InvokeResult> {
    if input.trim().is_empty() {
        return Err(AppError::BadRequest("input is required".into()));
    }
    let consumer: &str = &key.account_id;
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

    // Admission — rate limit and credit in one transaction. The reservation is
    // the worst case for this agent's shape (its loop steps plus nested
    // call_agent runs). A publisher calling their own agent is not a sale.
    let self_invoke = consumer == publisher;
    let job_id = Uuid::new_v4().to_string();
    let steps = billable_steps(state, id).await;
    let hold_id = match crate::billing::admit(
        &state.db,
        crate::billing::AdmissionRequest {
            key,
            job_id: &job_id,
            price_per_1k,
            max_output_tokens: state.config.invoke_max_output_tokens,
            steps,
            self_invoke,
        },
    )
    .await
    .map_err(AppError::Other)?
    {
        crate::billing::Admission::Held { hold_id, .. } => hold_id,
        crate::billing::Admission::RateLimited { per_min } => {
            return Err(AppError::TooManyRequests(format!(
                "rate limit of {per_min} requests per minute reached for this key"
            )));
        }
        crate::billing::Admission::InsufficientCredit {
            needed_usd,
            available_usd,
        } => {
            return Err(AppError::PaymentRequired(format!(
                "insufficient credit: this call reserves up to {needed_usd:.4} USD, {available_usd:.4} USD available"
            )));
        }
    };
    // Released automatically on any early return or cancellation below, unless
    // settlement takes it over.
    let guard = crate::billing::HoldGuard::new(state.db.clone(), hold_id);

    // Create the job already 'running' so the background worker skips it; we run
    // it inline for a synchronous response.
    let objective_id = Uuid::new_v4().to_string();
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
    // A consumer recalls the knowledge distilled from the publisher's memory
    // (never its raw episodes) plus their own fork, and writes only to the
    // fork. The publisher calling their own agent is just an owner run.
    let mode = if self_invoke {
        crate::agent::engine::MemoryMode::Owner
    } else {
        crate::agent::engine::MemoryMode::Consumer {
            account_id: consumer.to_string(),
        }
    };
    let outcome = crate::agent::engine::run_job(state, &claimed, &mode).await;

    // Tokens metered over the whole job chain (call_agent sub-runs included),
    // whatever the outcome: a failed run still spent them and must be visible.
    let (pt, ct) = chain_tokens(state, &job_id).await?;

    let canned = match outcome {
        Ok(crate::agent::engine::RunOutcome::Completed { canned }) => canned,
        Ok(crate::agent::engine::RunOutcome::AwaitingApproval) => {
            crate::queue::mark_failed(
                &state.db,
                &job_id,
                "agent requires interactive approval; not available via the synchronous invoke API",
            )
            .await
            .ok();
            settle_unbilled(state, guard, key, id, &publisher, &job_id, pt, ct).await?;
            return Err(AppError::BadRequest(
                "This agent requires human approval before acting and cannot be invoked via the synchronous API".into(),
            ));
        }
        Err(e) => {
            // Synchronous jobs are excluded from crash recovery: an unmarked
            // failure would leave the row `running` forever.
            crate::queue::mark_failed(&state.db, &job_id, &format!("{e:#}"))
                .await
                .ok();
            settle_unbilled(state, guard, key, id, &publisher, &job_id, pt, ct).await?;
            return Err(AppError::Other(anyhow::anyhow!("agent run failed: {e}")));
        }
    };

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

    // Nothing is charged for demo (canned) output, nor when the publisher calls
    // its own agent: a self-invoke is a test, not a sale.
    let (billed, publisher_usd) = compute_bill(ct, price_per_1k, rev_share, canned || self_invoke);

    // Usage row, ledger row, balance and hold in one transaction.
    let hold_id = guard.take();
    crate::billing::settle(
        &state.db,
        crate::billing::Settlement {
            hold_id: Some(&hold_id),
            key,
            agent_id: id,
            publisher_account: &publisher,
            job_id: &job_id,
            prompt_tokens: pt,
            completion_tokens: ct,
            billed_usd: billed,
            publisher_usd,
        },
    )
    .await
    .map_err(AppError::Other)?;

    Ok(InvokeResult {
        name,
        output_text,
        prompt_tokens: pt,
        completion_tokens: ct,
        billed_usd: billed,
        publisher_usd,
        demo: canned,
        self_invoke,
    })
}

/// A run that produced no deliverable is not charged, but the tokens it spent
/// are recorded (usage row with billed 0) and the reservation is consumed.
#[allow(clippy::too_many_arguments)]
async fn settle_unbilled(
    state: &AppState,
    guard: crate::billing::HoldGuard,
    key: &crate::billing::ConsumerKey,
    agent_id: &str,
    publisher: &str,
    job_id: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
) -> AppResult<()> {
    let hold_id = guard.take();
    crate::billing::settle(
        &state.db,
        crate::billing::Settlement {
            hold_id: Some(&hold_id),
            key,
            agent_id,
            publisher_account: publisher,
            job_id,
            prompt_tokens,
            completion_tokens,
            billed_usd: 0.0,
            publisher_usd: 0.0,
        },
    )
    .await
    .map_err(AppError::Other)
}

/// Prompt/completion tokens of a job and every sub-job under it.
async fn chain_tokens(state: &AppState, job_id: &str) -> AppResult<(i64, i64)> {
    let row: (i64, i64) = sqlx::query_as(
        r#"WITH RECURSIVE chain(id) AS (
             SELECT ?1
             UNION ALL
             SELECT j.id FROM jobs j JOIN chain ON j.parent_job_id = chain.id
           )
           SELECT COALESCE(SUM(t.prompt_tokens), 0), COALESCE(SUM(t.completion_tokens), 0)
           FROM token_usage t WHERE t.job_id IN (SELECT id FROM chain)"#,
    )
    .bind(job_id)
    .fetch_one(&state.db)
    .await?;
    Ok(row)
}

/// Steps that may produce billable output for this agent: its four loop steps,
/// one web search, and four more per agent it orchestrates with call_agent.
async fn billable_steps(state: &AppState, agent_id: &str) -> u32 {
    let options: Option<(String,)> = sqlx::query_as(
        "SELECT options FROM agent_step_configs WHERE agent_id = ? AND step_type = 'action'",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let targets = options
        .and_then(|(o,)| serde_json::from_str::<crate::domain::StepOptions>(&o).ok())
        .map(|o| {
            let params = &o.tool_params;
            let list = params
                .get("call_agents")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as u32)
                .unwrap_or(0);
            let single = u32::from(params.get("call_agent_id").is_some());
            list.max(single)
        })
        .unwrap_or(0);
    INVOKE_BILLABLE_STEPS + 4 * targets
}

/// Consumer charge and publisher share for `completion_tokens` at
/// `price_per_1k`. `free` (demo output, self-invoke) yields zero for both.
/// Never negative: a misconfigured price or share is clamped.
fn compute_bill(
    completion_tokens: i64,
    price_per_1k: f64,
    revenue_share: f64,
    free: bool,
) -> (f64, f64) {
    if free || completion_tokens <= 0 {
        return (0.0, 0.0);
    }
    let billed = (completion_tokens as f64 / 1000.0) * price_per_1k.max(0.0);
    let share = revenue_share.clamp(0.0, 1.0);
    (billed, billed * share)
}

#[cfg(test)]
mod tests {
    use super::compute_bill;

    #[test]
    fn bills_output_tokens_and_splits_the_share() {
        let (billed, publisher) = compute_bill(2_000, 0.5, 0.7, false);
        assert!((billed - 1.0).abs() < 1e-9);
        assert!((publisher - 0.7).abs() < 1e-9);
    }

    #[test]
    fn demo_and_self_invoke_are_free() {
        assert_eq!(compute_bill(2_000, 0.5, 0.7, true), (0.0, 0.0));
    }

    #[test]
    fn never_negative_and_share_is_clamped() {
        assert_eq!(compute_bill(-5, 0.5, 0.7, false), (0.0, 0.0));
        assert_eq!(compute_bill(1_000, -1.0, 0.7, false), (0.0, 0.0));
        let (billed, publisher) = compute_bill(1_000, 1.0, 1.5, false);
        assert!((billed - 1.0).abs() < 1e-9);
        assert!((publisher - 1.0).abs() < 1e-9, "share clamped to 1.0");
    }
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

/// `GET /api/v1/agents/:id/memory` — what this agent has learnt about the
/// calling consumer account: their fork only, never the publisher's memory.
pub async fn consumer_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let consumer = auth_key(&state, &headers).await?;
    ensure_published(&state, &id).await?;
    let entries = state
        .memory
        .list(&crate::memory::MemoryScope::consumer(
            &id,
            &consumer.account_id,
        ))
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "agent": id, "memories": entries })))
}

/// `DELETE /api/v1/agents/:id/memory` — erase the calling consumer's fork
/// (ICM topic + mirror). The publisher's memory is untouched; this is the
/// consumer's right-to-erasure switch.
pub async fn forget_consumer_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let consumer = auth_key(&state, &headers).await?;
    ensure_published(&state, &id).await?;
    let icm_failed = state
        .memory
        .forget(&crate::memory::MemoryScope::consumer(
            &id,
            &consumer.account_id,
        ))
        .await
        .map_err(AppError::Other)?;
    // `complete` is false when ICM did not confirm the erasure: the fork may
    // still be recalled there, and the call must be made again.
    Ok(Json(json!({
        "ok": true,
        "agent": id,
        "icm_failed": icm_failed,
        "complete": icm_failed == 0,
    })))
}

async fn ensure_published(state: &AppState, agent_id: &str) -> AppResult<()> {
    let row: Option<(String,)> = sqlx::query_as("SELECT visibility FROM agents WHERE id = ?")
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await?;
    match row {
        Some((v,)) if v == "public" => Ok(()),
        Some(_) => Err(AppError::BadRequest("agent is not published".into())),
        None => Err(AppError::NotFound("agent not found".into())),
    }
}
