//! Objectives: creating one enqueues a job for the agent engine.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateObjective {
    pub agent_id: String,
    pub title: String,
    pub prompt: String,
}

#[derive(Serialize)]
pub struct CreatedJob {
    pub objective_id: String,
    pub job_id: String,
}

/// `POST /api/objectives` — create an objective and enqueue its job.
pub async fn create(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Json(body): Json<CreateObjective>,
) -> AppResult<Json<CreatedJob>> {
    if body.prompt.trim().is_empty() {
        return Err(AppError::BadRequest("prompt is required".into()));
    }

    // Validate the agent exists.
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM agents WHERE id = ?")
        .bind(&body.agent_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("agent not found".into()));
    }
    // Running an agent requires at least editor rights on it.
    crate::http::users::require_agent_role(&state, &body.agent_id, &me, "editor").await?;

    let objective_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();

    let mut tx = state.db.begin().await?;
    sqlx::query(
        r#"INSERT INTO objectives (id, account_id, agent_id, title, prompt)
           VALUES (?, ?, ?, ?, ?)"#,
    )
    .bind(&objective_id)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&body.agent_id)
    .bind(&body.title)
    .bind(&body.prompt)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO jobs (id, objective_id, agent_id, status)
           VALUES (?, ?, ?, 'queued')"#,
    )
    .bind(&job_id)
    .bind(&objective_id)
    .bind(&body.agent_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    state
        .events
        .publish(crate::agent::JobEvent::status(&job_id, "queued", "job queued"));

    Ok(Json(CreatedJob { objective_id, job_id }))
}

/// `GET /api/objectives` — list recent objectives (for the demo prefill).
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    #[derive(Serialize, sqlx::FromRow)]
    struct Row {
        id: String,
        agent_id: String,
        title: String,
        prompt: String,
        created_at: String,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT id, agent_id, title, prompt, created_at FROM objectives
         WHERE account_id = ? ORDER BY created_at DESC LIMIT 50",
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "objectives": rows })))
}
