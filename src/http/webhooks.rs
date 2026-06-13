//! Inbound webhooks. An external system (e.g. an email/invoice provider) posts
//! a payload that emits an event; any agent whose `trigger_on` matches is run.
//! For the demo: POST an invoice body to /api/webhooks/invoice.received.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};
use uuid::Uuid;

/// `POST /api/webhooks/:event` — body is the payload (raw text or JSON). Every
/// agent listening on `event` (trigger_on) gets a job with the payload.
pub async fn receive(
    State(state): State<AppState>,
    Path(event): Path<String>,
    body: String,
) -> AppResult<Json<Value>> {
    let listeners: Vec<(String, String)> =
        sqlx::query_as("SELECT id, name FROM agents WHERE trigger_on = ?")
            .bind(&event)
            .fetch_all(&state.db)
            .await?;

    let mut jobs = Vec::new();
    for (agent_id, agent_name) in &listeners {
        let objective_id = Uuid::new_v4().to_string();
        let job_id = Uuid::new_v4().to_string();
        let title = format!("Webhook: {event}");
        let prompt = format!(
            "An inbound '{event}' event was received. Process this payload and \
             return the requested result.\n\nPayload:\n{body}"
        );

        let mut tx = state.db.begin().await?;
        sqlx::query(
            r#"INSERT INTO objectives (id, account_id, agent_id, title, prompt)
               VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(&objective_id)
        .bind(DEFAULT_ACCOUNT_ID)
        .bind(agent_id)
        .bind(&title)
        .bind(&prompt)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO jobs (id, objective_id, agent_id, status, trigger_event)
               VALUES (?, ?, ?, 'queued', ?)"#,
        )
        .bind(&job_id)
        .bind(&objective_id)
        .bind(agent_id)
        .bind(&event)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        state
            .events
            .publish(crate::agent::JobEvent::status(&job_id, "queued", "webhook trigger"));
        jobs.push(json!({ "job_id": job_id, "agent": agent_name }));
    }

    Ok(Json(json!({ "event": event, "triggered": jobs })))
}
