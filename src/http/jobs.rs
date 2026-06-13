//! Jobs: list, detail (steps + approvals + report), and the live SSE event feed.

use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use serde::Serialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

#[derive(Serialize, sqlx::FromRow)]
struct JobRow {
    id: String,
    agent_id: String,
    status: String,
    error: Option<String>,
    created_at: String,
    title: Option<String>,
}

/// `GET /api/jobs` — list recent jobs with their objective title.
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows = sqlx::query_as::<_, JobRow>(
        r#"SELECT j.id, j.agent_id, j.status, j.error, j.created_at, o.title
           FROM jobs j LEFT JOIN objectives o ON o.id = j.objective_id
           ORDER BY j.created_at DESC LIMIT 100"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "jobs": rows })))
}

/// `GET /api/jobs/:id` — full detail: job, steps, pending approval, report.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let job = sqlx::query_as::<_, JobRow>(
        r#"SELECT j.id, j.agent_id, j.status, j.error, j.created_at, o.title
           FROM jobs j LEFT JOIN objectives o ON o.id = j.objective_id
           WHERE j.id = ?"#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let Some(job) = job else {
        return Err(crate::error::AppError::NotFound("job not found".into()));
    };

    #[derive(Serialize, sqlx::FromRow)]
    struct StepRow {
        step_type: String,
        status: String,
        input: String,
        output: String,
        position: i64,
        finished_at: Option<String>,
    }
    let steps = sqlx::query_as::<_, StepRow>(
        "SELECT step_type, status, input, output, position, finished_at
         FROM steps WHERE job_id = ? ORDER BY position",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    #[derive(Serialize, sqlx::FromRow)]
    struct ApprovalRow {
        id: String,
        status: String,
        summary: String,
        payload: String,
        created_at: String,
    }
    let approvals = sqlx::query_as::<_, ApprovalRow>(
        "SELECT id, status, summary, payload, created_at
         FROM approvals WHERE job_id = ? ORDER BY created_at DESC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    // The report is the Restitution step output text, if present.
    let report = steps
        .iter()
        .find(|s| s.step_type == "restitution")
        .and_then(|s| serde_json::from_str::<Value>(&s.output).ok())
        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from));

    Ok(Json(json!({
        "job": job,
        "steps": steps,
        "approvals": approvals,
        "report": report,
    })))
}

/// `GET /api/jobs/:id/events` — Server-Sent Events stream of live progress.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let job_id = id.clone();
        match item {
            Ok(ev) if ev.job_id == job_id => Some(Ok(Event::default()
                .event("progress")
                .data(serde_json::to_string(&ev).unwrap_or_default()))),
            _ => None,
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
