//! Background worker: polls the queue, runs jobs through the engine, and keeps
//! going. A single worker is enough for the demo (SQLite single writer).

use super::engine;
use crate::queue;
use crate::state::AppState;
use std::time::Duration;

/// Spawn the worker loop. Returns immediately; the loop runs in the background.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        // Requeue jobs left running by a previous crashed process.
        match queue::recover_orphans(&state.db).await {
            Ok(n) if n > 0 => tracing::info!(recovered = n, "requeued orphaned jobs"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "orphan recovery failed"),
        }

        tracing::info!("job worker started");
        loop {
            match queue::claim_next(&state.db).await {
                Ok(Some(job)) => {
                    tracing::info!(job_id = %job.id, "claimed job");
                    if let Err(e) = engine::run_job(&state, &job, false).await {
                        tracing::error!(job_id = %job.id, error = %e, "job run failed");
                        engine::fail(&state, &job.id, &e).await;
                    }
                }
                Ok(None) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to claim job");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}
