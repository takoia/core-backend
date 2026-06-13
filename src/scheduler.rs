//! Recurring scheduler: periodically enqueues jobs for due schedules so a
//! full-auto agent loops and accumulates ICM memory over time. Supports a cron
//! expression or a simple `interval_seconds` (for fast local learning tests).

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::db::Db;
use crate::state::AppState;
use anyhow::Result;
use chrono::Utc;
use cron::Schedule;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

/// How often the scheduler wakes to check for due schedules.
const TICK: Duration = Duration::from_secs(10);

#[derive(sqlx::FromRow)]
struct ScheduleRow {
    id: String,
    agent_id: String,
    title: String,
    prompt: String,
    cron_expr: String,
    interval_seconds: Option<i64>,
    next_run_at: Option<String>,
}

/// Spawn the scheduler loop.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tracing::info!("scheduler started");
        loop {
            if let Err(e) = tick(&state).await {
                tracing::warn!(error = %e, "scheduler tick failed");
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn tick(state: &AppState) -> Result<()> {
    let now = Utc::now();
    let now_iso = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let due = sqlx::query_as::<_, ScheduleRow>(
        r#"SELECT id, agent_id, title, prompt, cron_expr, interval_seconds, next_run_at
           FROM schedules
           WHERE enabled = 1 AND (next_run_at IS NULL OR next_run_at <= ?)"#,
    )
    .bind(&now_iso)
    .fetch_all(&state.db)
    .await?;

    for s in due {
        // Avoid pile-up: skip if this agent already has an active job.
        if agent_has_active_job(&state.db, &s.agent_id).await? {
            continue;
        }
        enqueue_run(&state.db, &s).await?;
        let next = compute_next(&s, now);
        sqlx::query(
            r#"UPDATE schedules
               SET last_run_at = ?, next_run_at = ?, run_count = run_count + 1
               WHERE id = ?"#,
        )
        .bind(&now_iso)
        .bind(&next)
        .bind(&s.id)
        .execute(&state.db)
        .await?;
        tracing::info!(schedule = %s.id, agent = %s.agent_id, next = %next, "scheduled run enqueued");
    }
    Ok(())
}

async fn agent_has_active_job(db: &Db, agent_id: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"SELECT 1 FROM jobs
           WHERE agent_id = ? AND status IN ('queued','running','awaiting_approval') LIMIT 1"#,
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

async fn enqueue_run(db: &Db, s: &ScheduleRow) -> Result<()> {
    let objective_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();
    let title = if s.title.is_empty() {
        "Scheduled run".to_string()
    } else {
        s.title.clone()
    };

    let mut tx = db.begin().await?;
    sqlx::query(
        r#"INSERT INTO objectives (id, account_id, agent_id, title, prompt)
           VALUES (?, ?, ?, ?, ?)"#,
    )
    .bind(&objective_id)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&s.agent_id)
    .bind(&title)
    .bind(&s.prompt)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO jobs (id, objective_id, agent_id, status, trigger_event)
           VALUES (?, ?, ?, 'queued', 'schedule')"#,
    )
    .bind(&job_id)
    .bind(&objective_id)
    .bind(&s.agent_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Compute the next run time as an ISO-8601 string.
fn compute_next(s: &ScheduleRow, now: chrono::DateTime<Utc>) -> String {
    if let Some(secs) = s.interval_seconds.filter(|v| *v > 0) {
        return (now + chrono::Duration::seconds(secs))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
    }
    // Cron path.
    if let Ok(schedule) = Schedule::from_str(&s.cron_expr) {
        if let Some(next) = schedule.upcoming(Utc).next() {
            return next.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        }
    }
    // Fallback: one hour out, so a malformed schedule does not hot-loop.
    let _ = &s.next_run_at;
    (now + chrono::Duration::hours(1))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}
