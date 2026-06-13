//! Schedules: recurring autonomous runs for an agent (the learning loop).

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow)]
struct ScheduleRow {
    id: String,
    agent_id: String,
    title: String,
    prompt: String,
    cron_expr: String,
    interval_seconds: Option<i64>,
    enabled: i64,
    run_count: i64,
    last_run_at: Option<String>,
    next_run_at: Option<String>,
}

/// `GET /api/schedules` — list schedules.
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query_as::<_, ScheduleRow>(
        r#"SELECT id, agent_id, title, prompt, cron_expr, interval_seconds, enabled,
                  run_count, last_run_at, next_run_at
           FROM schedules ORDER BY created_at DESC"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "schedules": rows })))
}

#[derive(Deserialize)]
pub struct CreateSchedule {
    pub agent_id: String,
    #[serde(default)]
    pub title: String,
    pub prompt: String,
    /// Cron expression (used when interval_seconds is absent).
    #[serde(default = "default_cron")]
    pub cron_expr: String,
    /// Simple fixed interval; overrides cron when set. Great for fast tests.
    #[serde(default)]
    pub interval_seconds: Option<i64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_cron() -> String {
    "0 0 * * * *".into() // hourly
}
fn default_true() -> bool {
    true
}

/// `POST /api/schedules` — create a recurring schedule (runs immediately, then
/// on the interval/cron).
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateSchedule>,
) -> AppResult<Json<Value>> {
    if body.prompt.trim().is_empty() {
        return Err(AppError::BadRequest("prompt is required".into()));
    }
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM agents WHERE id = ?")
        .bind(&body.agent_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("agent not found".into()));
    }

    // One recurring schedule per agent: drop any existing ones first so repeated
    // saves don't pile up duplicate loops that flood the worker.
    sqlx::query("DELETE FROM schedules WHERE agent_id = ?")
        .bind(&body.agent_id)
        .execute(&state.db)
        .await?;

    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO schedules
             (id, agent_id, title, prompt, cron_expr, interval_seconds, enabled, next_run_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, NULL)"#, // NULL next_run_at -> runs on next tick
    )
    .bind(&id)
    .bind(&body.agent_id)
    .bind(&body.title)
    .bind(&body.prompt)
    .bind(&body.cron_expr)
    .bind(body.interval_seconds)
    .bind(body.enabled as i64)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "id": id })))
}

/// `POST /api/schedules/:id/toggle` — enable/disable.
pub async fn toggle(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let res = sqlx::query(
        "UPDATE schedules SET enabled = 1 - enabled WHERE id = ? RETURNING enabled",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    if res.is_none() {
        return Err(AppError::NotFound("schedule not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/schedules/:id` — remove a schedule.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM schedules WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
