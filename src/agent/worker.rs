//! Background worker: polls the queue and runs jobs through the engine.
//!
//! Jobs run concurrently, each in its own task, bounded by a semaphore. A
//! single perpetual loop job therefore can no longer block a freshly launched
//! manual run: as long as a slot is free, a queued job is claimed and started
//! within one poll interval (~500ms). SQLite serializes writes (WAL +
//! busy_timeout), so the bound caps parallel `claude -p` calls, not DB writers.

use super::engine;
use crate::queue;
use crate::state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Maximum number of jobs executing at the same time.
const MAX_CONCURRENT_JOBS: usize = 4;

/// How often the stale synchronous-job sweep runs.
const STALE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Crash recovery, run BEFORE the HTTP server accepts requests: requeue
/// background jobs left `running`, and fail every synchronous job — none can
/// have survived the restart, its handler died with the previous process.
/// Doing this before serving means the age-0 sweep can never hit a job that a
/// fresh request just created.
pub async fn recover(state: &AppState) {
    match queue::recover_orphans(&state.db).await {
        Ok(n) if n > 0 => tracing::info!(recovered = n, "requeued orphaned jobs"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "orphan recovery failed"),
    }
    match queue::fail_stale_synchronous(&state.db, 0).await {
        Ok(n) if n > 0 => tracing::info!(
            failed = n,
            "failed synchronous jobs left over from the previous process"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "stale synchronous job sweep failed"),
    }
    // A credit hold belongs to an invoke that died with the previous process.
    match crate::billing::sweep_stale_holds(&state.db, 0).await {
        Ok(n) if n > 0 => tracing::info!(
            released = n,
            "released credit holds left over from the previous process"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "stale credit hold sweep failed"),
    }
}

/// Spawn the worker loop and the periodic stale-job sweep. Returns immediately.
pub fn spawn(state: AppState) {
    {
        let db = state.db.clone();
        let max_age = state.config.sync_job_max_secs;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(STALE_SWEEP_INTERVAL).await;
                match queue::fail_stale_synchronous(&db, max_age).await {
                    Ok(n) if n > 0 => tracing::warn!(failed = n, "failed stale synchronous jobs"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "stale synchronous job sweep failed"),
                }
                match crate::billing::sweep_stale_holds(&db, max_age).await {
                    Ok(n) if n > 0 => tracing::warn!(released = n, "released stale credit holds"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "stale credit hold sweep failed"),
                }
            }
        });
    }
    tokio::spawn(async move {
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS));
        tracing::info!(max_concurrent = MAX_CONCURRENT_JOBS, "job worker started");
        loop {
            // Block until a slot frees up, so we never claim a job we can't run.
            let permit = match permits.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break, // semaphore closed: shutting down
            };
            match queue::claim_next(&state.db).await {
                Ok(Some(job)) => {
                    tracing::info!(job_id = %job.id, "claimed job");
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            engine::run_job(&state, &job, &engine::MemoryMode::Owner).await
                        {
                            // Background runs fail loudly; the job row records why.
                            tracing::error!(job_id = %job.id, error = %e, "job run failed");
                            engine::fail(&state, &job.id, &e).await;
                        }
                        drop(permit); // release the slot when the job finishes
                    });
                }
                Ok(None) => {
                    drop(permit);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    drop(permit);
                    tracing::error!(error = %e, "failed to claim job");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}
