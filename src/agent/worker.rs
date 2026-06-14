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

/// Spawn the worker loop. Returns immediately; the loop runs in the background.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        // Requeue jobs left running by a previous crashed process.
        match queue::recover_orphans(&state.db).await {
            Ok(n) if n > 0 => tracing::info!(recovered = n, "requeued orphaned jobs"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "orphan recovery failed"),
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
