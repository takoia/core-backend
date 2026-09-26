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

/// A synchronous run is at most four steps of `STEP_TIMEOUT`; anything older
/// than this still `running` has lost its handler.
const STALE_SYNC_JOB_SECS: i64 = 4 * crate::llm::claude_cli::STEP_TIMEOUT.as_secs() as i64 + 60;

/// How often the stale synchronous-job sweep runs.
const STALE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn the worker loop. Returns immediately; the loop runs in the background.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        // Requeue jobs left running by a previous crashed process.
        match queue::recover_orphans(&state.db).await {
            Ok(n) if n > 0 => tracing::info!(recovered = n, "requeued orphaned jobs"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "orphan recovery failed"),
        }
        // No synchronous job can have survived the restart: its handler is gone.
        match queue::fail_stale_synchronous(&state.db, 0).await {
            Ok(n) if n > 0 => tracing::info!(
                failed = n,
                "failed synchronous jobs left over from the previous process"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "stale synchronous job sweep failed"),
        }
        {
            let db = state.db.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(STALE_SWEEP_INTERVAL).await;
                    match queue::fail_stale_synchronous(&db, STALE_SYNC_JOB_SECS).await {
                        Ok(n) if n > 0 => {
                            tracing::warn!(failed = n, "failed stale synchronous jobs")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "stale synchronous job sweep failed"),
                    }
                }
            });
        }

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
                        if let Err(e) = engine::run_job(&state, &job, false).await {
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
