//! Agents CRUD, per-step configuration (the customization differentiator),
//! marketplace publishing, and memory listing.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::domain::StepType;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow)]
struct AgentRow {
    id: String,
    name: String,
    description: String,
    autonomy_level: String,
    expertise_domain: String,
    visibility: String,
    price_per_run_usd: f64,
    runs_count: i64,
    created_at: String,
    author: String,
    trigger_on: Option<String>,
    emit: String,
    icon: String,
    persona: String,
}

/// `GET /api/agents` — list this account's agents.
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query_as::<_, AgentRow>(
        r#"SELECT id, name, description, autonomy_level, expertise_domain, visibility,
                  price_per_run_usd, runs_count, created_at, author, trigger_on, emit, icon, persona
           FROM agents WHERE account_id = ? ORDER BY created_at DESC"#,
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "agents": rows })))
}

/// `GET /api/agents/:id` — agent detail with its four step configs.
pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> AppResult<Json<Value>> {
    let agent = sqlx::query_as::<_, AgentRow>(
        r#"SELECT id, name, description, autonomy_level, expertise_domain, visibility,
                  price_per_run_usd, runs_count, created_at, author, trigger_on, emit, icon, persona
           FROM agents WHERE id = ?"#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".into()))?;

    #[derive(Serialize, sqlx::FromRow)]
    struct StepConfig {
        step_type: String,
        system_prompt: String,
        options: String,
        position: i64,
    }
    let steps = sqlx::query_as::<_, StepConfig>(
        "SELECT step_type, system_prompt, options, position
         FROM agent_step_configs WHERE agent_id = ? ORDER BY position",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({ "agent": agent, "steps": steps })))
}

#[derive(Deserialize)]
pub struct CreateAgent {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub expertise_domain: String,
    #[serde(default)]
    pub autonomy_level: Option<String>,
}

/// `POST /api/agents` — create an agent with default step configs.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateAgent>,
) -> AppResult<Json<Value>> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    let autonomy = body
        .autonomy_level
        .filter(|a| a == "full_auto" || a == "confirm_before_action")
        .unwrap_or_else(|| "confirm_before_action".into());

    let id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        r#"INSERT INTO agents (id, account_id, name, description, autonomy_level, expertise_domain)
           VALUES (?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&body.name)
    .bind(&body.description)
    .bind(&autonomy)
    .bind(&body.expertise_domain)
    .execute(&mut *tx)
    .await?;

    for (pos, step) in StepType::ALL.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO agent_step_configs (id, agent_id, step_type, system_prompt, options, position)
               VALUES (?, ?, ?, '', '{}', ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&id)
        .bind(step.as_str())
        .bind(pos as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct UpdateAgent {
    pub name: Option<String>,
    pub description: Option<String>,
    pub expertise_domain: Option<String>,
    pub autonomy_level: Option<String>,
    pub price_per_run_usd: Option<f64>,
}

/// `PUT /api/agents/:id` — update agent settings (autonomy, pricing, …).
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAgent>,
) -> AppResult<Json<Value>> {
    sqlx::query(
        r#"UPDATE agents SET
             name = COALESCE(?, name),
             description = COALESCE(?, description),
             expertise_domain = COALESCE(?, expertise_domain),
             autonomy_level = COALESCE(?, autonomy_level),
             price_per_run_usd = COALESCE(?, price_per_run_usd),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
           WHERE id = ?"#,
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(&body.expertise_domain)
    .bind(body.autonomy_level.as_ref().filter(|a| {
        a.as_str() == "full_auto" || a.as_str() == "confirm_before_action"
    }))
    .bind(body.price_per_run_usd)
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct StepConfigInput {
    pub step_type: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub options: Value,
}

#[derive(Deserialize)]
pub struct UpdateSteps {
    pub steps: Vec<StepConfigInput>,
}

/// `PUT /api/agents/:id/steps` — edit the per-step prompts/options.
pub async fn update_steps(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateSteps>,
) -> AppResult<Json<Value>> {
    let mut tx = state.db.begin().await?;
    for step in &body.steps {
        let options = if step.options.is_null() {
            "{}".to_string()
        } else {
            step.options.to_string()
        };
        sqlx::query(
            r#"UPDATE agent_step_configs
               SET system_prompt = ?, options = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
               WHERE agent_id = ? AND step_type = ?"#,
        )
        .bind(&step.system_prompt)
        .bind(&options)
        .bind(&id)
        .bind(&step.step_type)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct PublishInput {
    /// "public" or "private".
    pub visibility: String,
    pub price_per_run_usd: Option<f64>,
    /// Price charged to consumers per 1k outgoing (completion) tokens.
    pub price_per_1k_output_tokens: Option<f64>,
    /// Share of revenue paid to the publisher (0..1).
    pub revenue_share: Option<f64>,
}

/// `POST /api/agents/:id/publish` — make the expert agent public or private.
pub async fn publish(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PublishInput>,
) -> AppResult<Json<Value>> {
    let visibility = match body.visibility.as_str() {
        "public" | "private" => body.visibility.as_str(),
        _ => return Err(AppError::BadRequest("visibility must be public or private".into())),
    };
    sqlx::query(
        r#"UPDATE agents SET
             visibility = ?,
             price_per_run_usd = COALESCE(?, price_per_run_usd),
             price_per_1k_output_tokens = COALESCE(?, price_per_1k_output_tokens),
             revenue_share = COALESCE(?, revenue_share),
             published_at = CASE WHEN ? = 'public' THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') ELSE published_at END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
           WHERE id = ?"#,
    )
    .bind(visibility)
    .bind(body.price_per_run_usd)
    .bind(body.price_per_1k_output_tokens)
    .bind(body.revenue_share)
    .bind(visibility)
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true, "visibility": visibility })))
}

/// `POST /api/agents/import` — import a declarative agent from a TOML body.
pub async fn import_toml(State(state): State<AppState>, body: String) -> AppResult<Json<Value>> {
    let id = crate::agentdef::import(&state.db, DEFAULT_ACCOUNT_ID, &body)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "id": id })))
}

/// `GET /api/agents/:id/export` — export the agent as a TOML definition.
pub async fn export_toml(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let toml = crate::agentdef::export(&state.db, &id)
        .await
        .map_err(AppError::Other)?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        toml,
    ))
}

/// `DELETE /api/agents/:id` — remove an agent and its configs (cascade).
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM agents WHERE id = ? AND account_id = ?")
        .bind(&id)
        .bind(DEFAULT_ACCOUNT_ID)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AddMemory {
    pub content: String,
    #[serde(default = "default_mem_key")]
    pub key: String,
}

fn default_mem_key() -> String {
    "demonstration".into()
}

/// `POST /api/agents/:id/memory` — add a piece of knowledge to an agent's
/// permanent memory (e.g. confirmed info from a screen-recording demonstration),
/// improving the agent over time.
pub async fn add_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AddMemory>,
) -> AppResult<Json<Value>> {
    if body.content.trim().is_empty() {
        return Err(AppError::BadRequest("content is required".into()));
    }
    state
        .memory
        .store(&id, &body.key, &body.content)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/agents/:id/memories` — the agent's accumulated expertise.
pub async fn memories(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let items = state.memory.list(&id).await.map_err(AppError::Other)?;
    Ok(Json(json!({ "memories": items })))
}

#[derive(Deserialize)]
pub struct ScaffoldInput {
    pub description: String,
}

/// `POST /api/agents/scaffold` — from a one-line description, generate the 4
/// step definitions (analyse/decision/action/restitution) + a persona via
/// `claude -p`. The builder fills the boxes with the result.
pub async fn scaffold(
    State(state): State<AppState>,
    Json(body): Json<ScaffoldInput>,
) -> AppResult<Json<Value>> {
    if body.description.trim().is_empty() {
        return Err(AppError::BadRequest("description is required".into()));
    }
    let prompt = format!(
        "You design autonomous agents that run a 4-step loop: analyse, decision, \
         action, restitution. For this agent description, write a concise system \
         prompt (2-4 sentences, imperative) DEFINING what each step must do, plus \
         a short 'persona' (the agent's identity/voice/expertise). Answer in the \
         same language as the description. Return ONLY a JSON object with keys \
         persona, analyse, decision, action, restitution. No prose, no code \
         fences.\n\nAgent description:\n{}",
        body.description.trim()
    );

    let mut cmd = tokio::process::Command::new("claude");
    cmd.arg("-p")
        .arg("--output-format")
        .arg("json")
        .arg("--permission-mode")
        .arg("bypassPermissions");
    if let Some(token) = &state.config.claude_max_token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Other(anyhow::anyhow!("failed to spawn claude: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(prompt.as_bytes()).await;
        drop(stdin);
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Other(e.into()))?;
    if !output.status.success() {
        return Err(AppError::Other(anyhow::anyhow!(
            "claude failed: {}",
            String::from_utf8_lossy(&output.stderr).chars().take(200).collect::<String>()
        )));
    }
    let parsed: Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim())
        .map_err(|e| AppError::Other(anyhow::anyhow!("invalid claude output: {e}")))?;
    let text = parsed.get("result").and_then(|v| v.as_str()).unwrap_or_default();
    // The model returns a JSON object; tolerate surrounding prose/fences.
    let slice = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => text,
    };
    let fields: Value = serde_json::from_str(slice)
        .unwrap_or_else(|_| json!({ "persona": "", "analyse": text, "decision": "", "action": "", "restitution": "" }));
    Ok(Json(fields))
}

/// ICM memories with native importance metadata (weight, access_count) so the
/// memory map can size/color entries by real importance relative to others.
pub async fn icm_memories(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    // Keyword query from the agent's name + domain so its memories surface.
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT name, expertise_domain FROM agents WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| AppError::Other(e.into()))?;
    let query = row
        .map(|(n, e)| format!("{n} {e}"))
        .unwrap_or_else(|| "memory".into());
    let entries = state.memory.icm_entries(&id, &query, 30).await;
    Ok(Json(json!({ "entries": entries })))
}
