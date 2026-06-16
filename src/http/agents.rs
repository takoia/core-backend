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
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
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
    // The creator owns the agent (RBAC).
    sqlx::query("INSERT INTO agent_permissions (agent_id, user_id, role) VALUES (?, ?, 'owner')")
        .bind(&id)
        .bind(&me.id)
        .execute(&mut *tx)
        .await?;
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
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateSteps>,
) -> AppResult<Json<Value>> {
    crate::http::users::require_agent_role(&state, &id, &me, "editor").await?;
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

/// Run a one-shot `claude -p` generation and return the inner JSON object the
/// model produced. Delegates to the sandboxed one-shot helper so the call is
/// confined by the active execution sandbox like every other run.
async fn generate_json(state: &AppState, prompt: &str) -> AppResult<Value> {
    crate::llm::oneshot::generate_json(state, prompt)
        .await
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("AI generation failed or returned no JSON")))
}

#[derive(Deserialize)]
pub struct ImportSoul {
    /// Raw external agent definition (OpenClaw `SOUL.md`, Hermes config, …).
    pub soul: String,
    /// When true, publish the imported agent to the marketplace immediately.
    /// Defaults to false: imports stay private until the owner opts in.
    #[serde(default)]
    pub publish: bool,
    /// Price charged per 1k output tokens when published.
    pub price_per_1k_output_tokens: Option<f64>,
}

/// `POST /api/agents/import-soul` — translate an external agent definition
/// (an OpenClaw `SOUL.md` file or a Hermes agent config) into a native TakoIA
/// agent running the 4-step loop. The agent is created PRIVATE and owned by the
/// caller; pass `publish: true` to list it on the marketplace right away.
pub async fn import_soul(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Json(body): Json<ImportSoul>,
) -> AppResult<Json<Value>> {
    if body.soul.trim().is_empty() {
        return Err(AppError::BadRequest("agent definition is required".into()));
    }
    let prompt = format!(
        "Translate this external AI agent definition (an OpenClaw SOUL.md file or a \
         Hermes agent config) into a TakoIA agent that runs a 4-step loop: analyse, \
         decision, action, restitution. Map the source agent's identity, voice and \
         rules into a 'persona'; map its purpose into concise imperative system \
         prompts for each step. Also pick a short 'name', a one-line 'description', \
         and an 'expertise' domain. Answer in the same language as the source. \
         Return ONLY a JSON object with keys name, description, expertise, persona, \
         analyse, decision, action, restitution. No prose, no code fences.\n\n\
         Agent definition:\n{}",
        body.soul.trim()
    );
    let fields = generate_json(&state, &prompt).await?;
    let field = |k: &str| {
        fields
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let name = {
        let n = field("name");
        if n.trim().is_empty() {
            "Imported agent".to_string()
        } else {
            n
        }
    };
    let description = field("description");
    let expertise = field("expertise");
    let persona = field("persona");

    let id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        r#"INSERT INTO agents (id, account_id, name, description, autonomy_level, expertise_domain, persona)
           VALUES (?, ?, ?, ?, 'full_auto', ?, ?)"#,
    )
    .bind(&id)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&name)
    .bind(&description)
    .bind(&expertise)
    .bind(&persona)
    .execute(&mut *tx)
    .await?;

    for (pos, step) in StepType::ALL.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO agent_step_configs (id, agent_id, step_type, system_prompt, options, position)
               VALUES (?, ?, ?, ?, '{}', ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&id)
        .bind(step.as_str())
        .bind(field(step.as_str()))
        .bind(pos as i64)
        .execute(&mut *tx)
        .await?;
    }
    // The importer owns the agent (RBAC).
    sqlx::query("INSERT INTO agent_permissions (agent_id, user_id, role) VALUES (?, ?, 'owner')")
        .bind(&id)
        .bind(&me.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if body.publish {
        sqlx::query(
            r#"UPDATE agents SET
                 visibility = 'public',
                 price_per_1k_output_tokens = COALESCE(?, price_per_1k_output_tokens),
                 revenue_share = COALESCE(revenue_share, 0.7),
                 published_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
               WHERE id = ?"#,
        )
        .bind(body.price_per_1k_output_tokens)
        .bind(&id)
        .execute(&state.db)
        .await?;
    }

    Ok(Json(json!({ "id": id, "name": name, "published": body.publish })))
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
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    crate::http::users::require_agent_role(&state, &id, &me, "owner").await?;
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

    let fields = crate::llm::oneshot::generate_json(&state, &prompt)
        .await
        .unwrap_or_else(|| json!({ "persona": "", "analyse": "", "decision": "", "action": "", "restitution": "" }));
    Ok(Json(fields))
}

/// `POST /api/agents/:id/evolve-persona` — regenerate the agent's persona from
/// its accumulated memories, so its identity grows with what it has lived
/// (including the tone of how the user has treated it). Two agents that start
/// from the same persona diverge as their memories diverge.
pub async fn evolve_persona(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT name, expertise_domain, persona FROM agents WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let Some((name, expertise, current_persona)) = row else {
        return Err(AppError::NotFound("agent not found".into()));
    };

    // Gather what the agent has accumulated. recall() falls back to the agent's
    // top-weight memories when the keyword query misses.
    let memories = state
        .memory
        .recall(&id, "interactions tone style how the user treats me preferences", 16)
        .await;
    if memories.trim().is_empty() {
        return Err(AppError::BadRequest(
            "agent has no memories yet — nothing to evolve from".into(),
        ));
    }

    let prompt = format!(
        "You evolve an AI agent's PERSONA based on what it has lived. Below is its \
         current persona and the memories it has accumulated — including the tone \
         of how the user has interacted with it. Rewrite the persona so it reflects \
         this lived experience: if the user has been harsh or insulting, the agent \
         grows more guarded, terse and defensive; if the user has been kind and \
         appreciative, it grows warmer, proactive and trusting. Keep the agent's \
         core expertise intact. Answer in the same language as the current persona, \
         2-4 sentences, written in the agent's own voice. Return ONLY the new \
         persona text, no preamble, no quotes.\n\n\
         Agent name: {name}\nExpertise: {expertise}\n\nCurrent persona:\n{current_persona}\n\n\
         Accumulated memories:\n{memories}"
    );

    let new_persona = crate::llm::oneshot::generate_text(&state, &prompt)
        .await
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("AI returned an empty persona")))?;

    sqlx::query("UPDATE agents SET persona = ? WHERE id = ?")
        .bind(&new_persona)
        .bind(&id)
        .execute(&state.db)
        .await?;

    Ok(Json(json!({ "persona": new_persona, "previous": current_persona })))
}

/// `GET /api/agents/:id/inner-state` — the agent's current "inner life": mood,
/// energy, familiarity, latest reflection, and its open commitments.
pub async fn inner_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let st: Option<(String, f64, i64, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT mood, energy, familiarity, reflection, emotions, updated_at FROM agent_state WHERE agent_id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;

    #[derive(Serialize, sqlx::FromRow)]
    struct Commitment {
        description: String,
        due_at: Option<String>,
        done: i64,
    }
    let commitments = sqlx::query_as::<_, Commitment>(
        "SELECT description, due_at, done FROM agent_commitments
         WHERE agent_id = ? ORDER BY created_at DESC LIMIT 10",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    let emotions = |e: Option<String>| -> Value {
        e.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null)
    };
    let state_json = match st {
        Some((mood, energy, familiarity, reflection, emo, updated_at)) => json!({
            "mood": mood,
            "energy": energy,
            "familiarity": familiarity,
            "reflection": reflection,
            "emotions": emotions(emo),
            "updated_at": updated_at,
        }),
        None => json!({
            "mood": "curious",
            "energy": 0.6,
            "familiarity": 0,
            "reflection": null,
            "emotions": null,
            "updated_at": null,
        }),
    };
    Ok(Json(json!({ "state": state_json, "commitments": commitments })))
}

/// `GET /api/agents/:id/personalization` — the per-agent feature toggles + Big Five.
pub async fn get_personalization(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let p = crate::agent::inner_life::personalization(&state.db, &id).await;
    Ok(Json(json!({
        "reflection": p.reflection,
        "emotions": p.emotions,
        "initiative": p.initiative,
        "commitments": p.commitments,
        "persona_evolution": p.persona_evolution,
        "personality": p.personality,
        "big_five": p.big_five,
    })))
}

#[derive(Deserialize)]
pub struct PersonalizationInput {
    pub reflection: Option<bool>,
    pub emotions: Option<bool>,
    pub initiative: Option<bool>,
    pub commitments: Option<bool>,
    pub persona_evolution: Option<bool>,
    pub personality: Option<bool>,
    pub big_five: Option<crate::agent::inner_life::BigFive>,
}

/// `PUT /api/agents/:id/personalization` — enable/disable features + set Big Five.
pub async fn set_personalization(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<PersonalizationInput>,
) -> AppResult<Json<Value>> {
    crate::http::users::require_agent_role(&state, &id, &me, "editor").await?;
    let cur = crate::agent::inner_life::personalization(&state.db, &id).await;
    let reflection = body.reflection.unwrap_or(cur.reflection);
    let emotions = body.emotions.unwrap_or(cur.emotions);
    let initiative = body.initiative.unwrap_or(cur.initiative);
    let commitments = body.commitments.unwrap_or(cur.commitments);
    let persona_evolution = body.persona_evolution.unwrap_or(cur.persona_evolution);
    let personality = body.personality.unwrap_or(cur.personality);
    let mut big_five = body.big_five.unwrap_or(cur.big_five);
    for v in [
        &mut big_five.openness,
        &mut big_five.conscientiousness,
        &mut big_five.extraversion,
        &mut big_five.agreeableness,
        &mut big_five.neuroticism,
    ] {
        *v = v.clamp(0.0, 1.0);
    }
    let bf_json = serde_json::to_string(&big_five).unwrap_or_default();
    sqlx::query(
        r#"INSERT INTO agent_personalization
             (agent_id, reflection, emotions, initiative, commitments, persona_evolution, personality, big_five)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(agent_id) DO UPDATE SET
             reflection = excluded.reflection,
             emotions = excluded.emotions,
             initiative = excluded.initiative,
             commitments = excluded.commitments,
             persona_evolution = excluded.persona_evolution,
             personality = excluded.personality,
             big_five = excluded.big_five,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(&id)
    .bind(reflection as i64)
    .bind(emotions as i64)
    .bind(initiative as i64)
    .bind(commitments as i64)
    .bind(persona_evolution as i64)
    .bind(personality as i64)
    .bind(&bf_json)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
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
