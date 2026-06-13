//! Approvals: the human-in-the-loop gate. Approving requeues the paused job.

use crate::error::{AppError, AppResult};
use crate::queue;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct Decision {
    /// "approve" or "reject".
    pub decision: String,
}

/// `POST /api/approvals/:id` — approve or reject a pending action.
pub async fn decide(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Decision>,
) -> AppResult<Json<Value>> {
    let new_status = match body.decision.as_str() {
        "approve" => "approved",
        "reject" => "rejected",
        _ => return Err(AppError::BadRequest("decision must be approve or reject".into())),
    };

    let row: Option<(String, String)> =
        sqlx::query_as("SELECT job_id, status FROM approvals WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    let Some((job_id, status)) = row else {
        return Err(AppError::NotFound("approval not found".into()));
    };
    if status != "pending" {
        return Err(AppError::Conflict(format!("approval already {status}")));
    }

    sqlx::query(
        "UPDATE approvals SET status = ?, decided_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?",
    )
    .bind(new_status)
    .bind(&id)
    .execute(&state.db)
    .await?;

    // Requeue so the worker resumes the job (the engine skips done steps).
    queue::requeue_approved(&state.db, &job_id).await?;
    state.events.publish(crate::agent::JobEvent::status(
        &job_id,
        "queued",
        format!("action {new_status}, resuming"),
    ));

    Ok(Json(json!({ "status": new_status, "job_id": job_id })))
}
