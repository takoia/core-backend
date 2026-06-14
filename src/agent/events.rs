//! Real-time job events broadcast to SSE subscribers (and later Discord).

use crate::db::Db;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};

/// Kind of progress event emitted during a run.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    JobStatus,
    StepStarted,
    StepCompleted,
    Log,
    ApprovalRequired,
    Report,
}

/// A single progress event for one job.
#[derive(Debug, Clone, Serialize)]
pub struct JobEvent {
    pub job_id: String,
    pub kind: EventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A row ready to be inserted into `event_log`. We pre-compute the derived
/// columns (id, kind wire string, serialized data) on the publishing side so
/// the background writer only performs the INSERT.
struct EventLogRow {
    id: String,
    job_id: String,
    kind: String,
    step_type: Option<String>,
    status: Option<String>,
    message: String,
    data: Option<String>,
}

impl EventLogRow {
    /// Build a persistable row from a `JobEvent`, mirroring exactly the columns
    /// and values that were inserted by the previous per-event spawn path.
    fn from_event(event: &JobEvent) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        // Serialize the kind enum to its snake_case wire string.
        let kind = serde_json::to_value(event.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let data = event.data.as_ref().map(|d| d.to_string());
        Self {
            id,
            job_id: event.job_id.clone(),
            kind,
            step_type: event.step_type.clone(),
            status: event.status.clone(),
            message: event.message.clone(),
            data,
        }
    }
}

/// A cloneable broadcast bus. SSE handlers subscribe and filter by `job_id`.
/// Every published event is also persisted to `event_log` for the audit trail.
///
/// `EventBus` stays `Clone` because both `broadcast::Sender` and
/// `mpsc::UnboundedSender` are cheaply cloneable handles. All clones feed the
/// single background writer task spawned in [`EventBus::new`].
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<JobEvent>,
    persist_tx: mpsc::UnboundedSender<EventLogRow>,
}

impl EventBus {
    pub fn new(db: Db) -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        let (persist_tx, persist_rx) = mpsc::unbounded_channel::<EventLogRow>();
        tokio::spawn(Self::persist_writer(db, persist_rx));
        Self { tx, persist_tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.tx.subscribe()
    }

    /// Publish an event: broadcast to live subscribers immediately, then hand
    /// the row off to the background writer for persistence to `event_log`.
    pub fn publish(&self, event: JobEvent) {
        let _ = self.tx.send(event.clone());
        self.persist(event);
    }

    /// Queue the event for persistence. The actual INSERT runs in the single
    /// background writer task, so the caller never blocks and bursts collapse
    /// into batched transactions. If the writer is gone, the send is dropped.
    fn persist(&self, event: JobEvent) {
        let row = EventLogRow::from_event(&event);
        let _ = self.persist_tx.send(row);
    }

    /// Single background task that drains the persistence channel and writes
    /// events to `event_log`. After each `recv()`, it opportunistically drains
    /// any already-queued rows with `try_recv()` and inserts the whole batch in
    /// one transaction, so bursts of events become a handful of transactions.
    async fn persist_writer(db: Db, mut rx: mpsc::UnboundedReceiver<EventLogRow>) {
        while let Some(first) = rx.recv().await {
            // Collect the head row plus everything already queued.
            let mut batch = vec![first];
            while let Ok(row) = rx.try_recv() {
                batch.push(row);
            }

            let mut tx = match db.begin().await {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::warn!(
                        "failed to begin transaction for {} event_log rows: {e}",
                        batch.len()
                    );
                    continue;
                }
            };

            let mut failed = false;
            for row in &batch {
                let res = sqlx::query(
                    r#"INSERT INTO event_log (id, job_id, kind, step_type, status, message, data)
                       VALUES (?, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(&row.id)
                .bind(&row.job_id)
                .bind(&row.kind)
                .bind(&row.step_type)
                .bind(&row.status)
                .bind(&row.message)
                .bind(&row.data)
                .execute(&mut *tx)
                .await;
                if let Err(e) = res {
                    tracing::warn!("failed to persist event to event_log: {e}");
                    failed = true;
                    break;
                }
            }

            if failed {
                if let Err(e) = tx.rollback().await {
                    tracing::warn!("failed to roll back event_log batch: {e}");
                }
            } else if let Err(e) = tx.commit().await {
                tracing::warn!(
                    "failed to commit {} event_log rows: {e}",
                    batch.len()
                );
            }
        }
    }
}

/// Helpers to build common events tersely.
impl JobEvent {
    pub fn status(job_id: &str, status: &str, message: impl Into<String>) -> Self {
        Self {
            job_id: job_id.to_string(),
            kind: EventKind::JobStatus,
            step_type: None,
            status: Some(status.to_string()),
            message: message.into(),
            data: None,
        }
    }

    pub fn step_started(job_id: &str, step: &str) -> Self {
        Self {
            job_id: job_id.to_string(),
            kind: EventKind::StepStarted,
            step_type: Some(step.to_string()),
            status: Some("running".to_string()),
            message: format!("{step} started"),
            data: None,
        }
    }

    pub fn step_completed(job_id: &str, step: &str, output: serde_json::Value) -> Self {
        Self {
            job_id: job_id.to_string(),
            kind: EventKind::StepCompleted,
            step_type: Some(step.to_string()),
            status: Some("done".to_string()),
            message: format!("{step} completed"),
            data: Some(output),
        }
    }

    pub fn log(job_id: &str, message: impl Into<String>) -> Self {
        Self {
            job_id: job_id.to_string(),
            kind: EventKind::Log,
            step_type: None,
            status: None,
            message: message.into(),
            data: None,
        }
    }

    pub fn approval_required(job_id: &str, approval_id: &str, summary: &str) -> Self {
        Self {
            job_id: job_id.to_string(),
            kind: EventKind::ApprovalRequired,
            step_type: Some("action".to_string()),
            status: Some("awaiting_approval".to_string()),
            message: summary.to_string(),
            data: Some(serde_json::json!({ "approval_id": approval_id })),
        }
    }

    pub fn report(job_id: &str, markdown: &str) -> Self {
        Self {
            job_id: job_id.to_string(),
            kind: EventKind::Report,
            step_type: Some("restitution".to_string()),
            status: Some("done".to_string()),
            message: "final report ready".to_string(),
            data: Some(serde_json::json!({ "markdown": markdown })),
        }
    }
}
