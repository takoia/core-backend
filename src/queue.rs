//! Postgres/SQLite-backed job queue.
//!
//! With SQLite (WAL, single writer) an atomic `UPDATE ... RETURNING` claims the
//! next queued job without `FOR UPDATE SKIP LOCKED`. The same shape ports to
//! Postgres by swapping in `SKIP LOCKED`.

use crate::db::Db;
use crate::domain::JobStatus;
use anyhow::Result;

/// A claimed job ready to run.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ClaimedJob {
    pub id: String,
    pub objective_id: String,
    pub agent_id: String,
}

/// Atomically claim the oldest queued job, marking it running.
pub async fn claim_next(db: &Db) -> Result<Option<ClaimedJob>> {
    let job = sqlx::query_as::<_, ClaimedJob>(
        r#"
        UPDATE jobs
        SET status = 'running',
            locked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            started_at = COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        WHERE id = (
            SELECT id FROM jobs
            WHERE status = 'queued'
            ORDER BY created_at
            LIMIT 1
        )
        RETURNING id, objective_id, agent_id
        "#,
    )
    .fetch_optional(db)
    .await?;
    Ok(job)
}

/// Resume a job that a human approval just released back to the runnable state.
pub async fn requeue_approved(db: &Db, job_id: &str) -> Result<()> {
    set_status(db, job_id, JobStatus::Queued).await
}

/// Update a job's status (and clear/set finished timestamp where relevant).
pub async fn set_status(db: &Db, job_id: &str, status: JobStatus) -> Result<()> {
    let finished = matches!(status, JobStatus::Done | JobStatus::Failed);
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = ?,
            finished_at = CASE WHEN ? THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE finished_at END,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        WHERE id = ?
        "#,
    )
    .bind(status.as_str())
    .bind(finished)
    .bind(job_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Mark a job failed with an error message.
pub async fn mark_failed(db: &Db, job_id: &str, error: &str) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'failed', error = ?,
            finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
        WHERE id = ?
        "#,
    )
    .bind(error)
    .bind(job_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Recover jobs left `running` by a previous crashed process: requeue them.
///
/// Synchronous inline-invoke jobs (`synchronous = 1`) are excluded: they are
/// executed synchronously within the HTTP handler and must never be requeued
/// by crash recovery.
pub async fn recover_orphans(db: &Db) -> Result<u64> {
    let res = sqlx::query(
        r#"UPDATE jobs SET status = 'queued', locked_at = NULL,
           updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
           WHERE status = 'running' AND synchronous = 0"#,
    )
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

/// Synchronous jobs (marketplace invokes, call_agent sub-runs) execute inside
/// the HTTP handler or the parent run. They are excluded from
/// `recover_orphans`, so one left `running` by a crashed handler or a server
/// restart would stay there forever and keep its agent "busy" for the
/// scheduler and the inner-life loop. Fail those older than `max_age_secs`.
pub async fn fail_stale_synchronous(db: &Db, max_age_secs: i64) -> Result<u64> {
    let res = sqlx::query(
        r#"UPDATE jobs SET status = 'failed',
           error = 'synchronous job abandoned (handler failed or server restarted)',
           finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
           updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
           WHERE synchronous = 1
             AND status IN ('running', 'awaiting_approval')
             AND COALESCE(started_at, created_at) < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?)"#,
    )
    .bind(format!("-{} seconds", max_age_secs.max(0)))
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}
