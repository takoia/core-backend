//! Event choreography: when an agent finishes a run it emits events; any agent
//! whose `trigger_on` matches an emitted event is enqueued automatically. This
//! is how agents are wired together — match one agent's `emit` to another's
//! `trigger.on`, no glue code.

use crate::agent::JobEvent;
use crate::state::AppState;
use anyhow::Result;
use uuid::Uuid;

/// Max chain length, to stop event cycles from running forever.
const MAX_CHAIN_DEPTH: i64 = 4;

/// Dispatch the events emitted by a finished job to any triggered agents.
pub async fn dispatch(
    state: &AppState,
    job_id: &str,
    emitting_agent_id: &str,
    report: &str,
) -> Result<()> {
    // What events does the finished agent emit, and how deep is this chain?
    let row: Option<(String, i64, String)> = sqlx::query_as(
        r#"SELECT a.emit, j.chain_depth, a.name
           FROM jobs j JOIN agents a ON a.id = j.agent_id
           WHERE j.id = ?"#,
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((emit_json, depth, emitter_name)) = row else {
        return Ok(());
    };

    let events: Vec<String> = serde_json::from_str(&emit_json).unwrap_or_default();
    if events.is_empty() {
        return Ok(());
    }
    if depth + 1 > MAX_CHAIN_DEPTH {
        state.events.publish(JobEvent::log(
            job_id,
            "event chain depth limit reached, not dispatching further",
        ));
        return Ok(());
    }

    for event in &events {
        // Find agents listening for this event (excluding the emitter itself).
        let listeners: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, name FROM agents WHERE trigger_on = ? AND id != ?",
        )
        .bind(event)
        .bind(emitting_agent_id)
        .fetch_all(&state.db)
        .await?;

        for (agent_id, agent_name) in listeners {
            let objective_id = Uuid::new_v4().to_string();
            let new_job_id = Uuid::new_v4().to_string();
            let title = format!("Triggered by '{event}' from {emitter_name}");
            let prompt = format!(
                "You were triggered by the event '{event}'. Upstream result:\n\n{report}"
            );

            let mut tx = state.db.begin().await?;
            sqlx::query(
                r#"INSERT INTO objectives (id, account_id, agent_id, title, prompt)
                   SELECT ?, account_id, ?, ?, ? FROM agents WHERE id = ?"#,
            )
            .bind(&objective_id)
            .bind(&agent_id)
            .bind(&title)
            .bind(&prompt)
            .bind(&agent_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"INSERT INTO jobs (id, objective_id, agent_id, status, chain_depth, trigger_event)
                   VALUES (?, ?, ?, 'queued', ?, ?)"#,
            )
            .bind(&new_job_id)
            .bind(&objective_id)
            .bind(&agent_id)
            .bind(depth + 1)
            .bind(event)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;

            state.events.publish(JobEvent::log(
                job_id,
                format!("emitted '{event}' -> triggered agent '{agent_name}'"),
            ));
            tracing::info!(event, agent_id = %agent_id, "choreography: triggered agent");
        }
    }
    Ok(())
}
