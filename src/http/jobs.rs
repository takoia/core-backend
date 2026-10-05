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
pub async fn list(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
) -> AppResult<Json<Value>> {
    let rows = sqlx::query_as::<_, JobRow>(
        r#"SELECT j.id, j.agent_id, j.status, j.error, j.created_at, o.title
           FROM jobs j
           JOIN agents a ON a.id = j.agent_id
           LEFT JOIN objectives o ON o.id = j.objective_id
           WHERE a.account_id = ?3
             AND (?1 = 1 OR j.agent_id IN (SELECT agent_id FROM agent_permissions WHERE user_id = ?2))
           ORDER BY j.created_at DESC LIMIT 100"#,
    )
    .bind(me.is_admin != 0)
    .bind(&me.id)
    .bind(&me.account_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "jobs": rows })))
}

/// `GET /api/jobs/:id` — full detail: job, steps, pending approval, report.
pub async fn get(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    crate::http::users::require_job_role(&state, &id, &me, "viewer").await?;
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

/// `POST /api/jobs/:id/feedback` — submit a correction for a job's output. The
/// correction is recorded as an episode of the memory that job's run wrote to
/// — the agent's own, or the consumer's fork for a marketplace run — so future
/// runs on that memory improve, and it can be erased like any other memory.
/// A run whose memory cannot be told (see [`run_scope`]) is not corrected:
/// 409, and nothing is stored. This is the "detect an error and improve the
/// agent" loop.
pub async fn feedback(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<FeedbackInput>,
) -> AppResult<Json<Value>> {
    crate::http::users::require_job_role(&state, &id, &me, "editor").await?;
    let row: Option<(String, String)> = sqlx::query_as(
        r#"SELECT j.agent_id, COALESCE(o.prompt, '')
           FROM jobs j LEFT JOIN objectives o ON o.id = j.objective_id
           WHERE j.id = ?"#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let Some((agent_id, context)) = row else {
        return Err(crate::error::AppError::NotFound("job not found".into()));
    };

    // The correction quotes the run (its prompt, its output): it goes where
    // that run's own memories went, under the same provenance. A consumer's
    // run is corrected in that consumer's fork, never in the publisher's
    // episodes — from where it would be distilled into what every buyer gets.
    let scope = run_scope(&state, &id, &agent_id).await?;
    let prov = crate::memory::Provenance::for_run(&scope, &id);
    state
        .memory
        .record_feedback(
            &scope,
            &prov,
            &context.chars().take(400).collect::<String>(),
            &body.predicted,
            &body.corrected,
            body.reason.as_deref().unwrap_or(""),
        )
        .await
        .map_err(crate::error::AppError::Other)?;

    state.events.publish(crate::agent::JobEvent::log(
        &id,
        "correction recorded — agent will improve",
    ));
    Ok(Json(json!({ "ok": true, "agent_id": agent_id })))
}

/// The memory scope the run of `job_id` wrote to. A marketplace invoke by
/// another account — and every `call_agent` sub-run under it — writes to that
/// consumer's fork of the agent it ran; anything else is an owner run.
///
/// Whose run it was is read from the job at the top of the chain: the account
/// recorded on it when it was invoked (`jobs.invoked_by`). An invoke older
/// than that column is told by its billing rows — its reservation while it
/// runs, its usage row afterwards — and, when it was abandoned before
/// settlement and has neither, by the fork its run wrote to.
///
/// When nothing says whose run it was, the answer is a refusal (409), never
/// the owner scope: a correction quotes the run's prompt, and a consumer's
/// prompt stored among the publisher's episodes would be distilled into what
/// every other buyer is given.
async fn run_scope(
    state: &AppState,
    job_id: &str,
    agent_id: &str,
) -> AppResult<crate::memory::MemoryScope> {
    use crate::memory::MemoryScope;
    let unknown = || {
        crate::error::AppError::Conflict(
            "the account this run was invoked for is not recorded (its invoke was \
             abandoned before it was settled, or is gone); its run cannot be corrected"
                .into(),
        )
    };
    // Up the `call_agent` chain: the job it ends on, and that job's agent's
    // account (the publisher).
    #[derive(sqlx::FromRow)]
    struct Top {
        id: String,
        parent_job_id: Option<String>,
        synchronous: i64,
        invoked_by: Option<String>,
        publisher: String,
    }
    let top: Option<Top> = sqlx::query_as(
        r#"WITH RECURSIVE chain(id, parent, depth) AS (
             SELECT id, parent_job_id, 0 FROM jobs WHERE id = ?1
             UNION ALL
             SELECT j.id, j.parent_job_id, chain.depth + 1
             FROM jobs j JOIN chain ON j.id = chain.parent
             WHERE chain.depth < 64
           )
           SELECT j.id, j.parent_job_id, j.synchronous, j.invoked_by,
                  a.account_id AS publisher
           FROM chain JOIN jobs j ON j.id = chain.id JOIN agents a ON a.id = j.agent_id
           ORDER BY chain.depth DESC LIMIT 1"#,
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(top) = top else {
        return Err(crate::error::AppError::NotFound("job not found".into()));
    };
    // The chain does not reach its first job (a parent is gone, or it is
    // deeper than any run goes): a sub-run of nobody knows whose invoke.
    if top.parent_job_id.is_some() {
        return Err(unknown());
    }
    let for_account = |account: &str| {
        if account == top.publisher {
            MemoryScope::owner(agent_id)
        } else {
            MemoryScope::consumer(agent_id, account)
        }
    };
    if let Some(account) = &top.invoked_by {
        return Ok(for_account(account));
    }
    // Only an invoke is created synchronous without a parent; everything
    // else at the top of a chain (objective, schedule, webhook, inner life)
    // is the publisher's own run.
    if top.synchronous == 0 {
        return Ok(MemoryScope::owner(agent_id));
    }
    // An invoke from before `invoked_by`: its billing rows, a consumer's
    // ahead of the publisher's own.
    let billed: Vec<String> = sqlx::query_scalar(
        r#"SELECT consumer_account FROM marketplace_usage WHERE job_id = ?1
           UNION ALL
           SELECT account_id FROM credit_hold WHERE job_id = ?1"#,
    )
    .bind(&top.id)
    .fetch_all(&state.db)
    .await?;
    if let Some(account) = billed
        .iter()
        .find(|account| **account != top.publisher)
        .or(billed.first())
    {
        return Ok(for_account(account));
    }
    // Abandoned before settlement: what its run stored, anywhere in the
    // chain, says which fork it wrote to. One fork, or it is not known.
    let forks: Vec<String> = sqlx::query_scalar(
        r#"WITH RECURSIVE tree(id, depth) AS (
             SELECT ?1, 0
             UNION ALL
             SELECT j.id, tree.depth + 1
             FROM jobs j JOIN tree ON j.parent_job_id = tree.id
             WHERE tree.depth < 64
           )
           SELECT DISTINCT m.consumer_account
           FROM memories m JOIN tree ON m.job_id = tree.id
           WHERE m.consumer_account IS NOT NULL
           LIMIT 2"#,
    )
    .bind(&top.id)
    .fetch_all(&state.db)
    .await?;
    match forks.as_slice() {
        [account] => Ok(for_account(account)),
        _ => Err(unknown()),
    }
}

#[derive(serde::Deserialize)]
pub struct FeedbackInput {
    pub predicted: String,
    pub corrected: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `GET /api/jobs/:id/events` — Server-Sent Events stream of live progress.
pub async fn events(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    crate::http::users::require_job_role(&state, &id, &me, "viewer").await?;
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let job_id = id.clone();
        match item {
            Ok(ev) if ev.job_id == job_id => Some(Ok(Event::default()
                .event("progress")
                .data(serde_json::to_string(&ev).unwrap_or_default()))),
            // The broadcast buffer overflowed for this slow client: tell it how
            // many events it lost so it can refetch the job instead of showing
            // a silently incomplete timeline.
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                Some(Ok(Event::default()
                    .event("lagged")
                    .data(json!({ "dropped": n }).to_string())))
            }
            _ => None,
        }
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}
