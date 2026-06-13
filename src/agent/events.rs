//! Real-time job events broadcast to SSE subscribers (and later Discord).

use crate::db::Db;
use serde::Serialize;
use tokio::sync::broadcast;

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

/// A cloneable broadcast bus. SSE handlers subscribe and filter by `job_id`.
/// Every published event is also persisted to `event_log` for the audit trail.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<JobEvent>,
    db: Db,
}

impl EventBus {
    pub fn new(db: Db) -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        Self { tx, db }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.tx.subscribe()
    }

    /// Publish an event: broadcast to live subscribers, then persist it to the
    /// `event_log` audit trail in the background.
    pub fn publish(&self, event: JobEvent) {
        let _ = self.tx.send(event.clone());
        self.persist(event);
    }

    /// Insert the event into `event_log`. Spawned so persistence never blocks
    /// the caller; failures are logged but do not affect the broadcast.
    fn persist(&self, event: JobEvent) {
        let db = self.db.clone();
        tokio::spawn(async move {
            let id = uuid::Uuid::new_v4().to_string();
            // Serialize the kind enum to its snake_case wire string.
            let kind = serde_json::to_value(event.kind)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            let data = event.data.as_ref().map(|d| d.to_string());
            let res = sqlx::query(
                r#"INSERT INTO event_log (id, job_id, kind, step_type, status, message, data)
                   VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(&event.job_id)
            .bind(&kind)
            .bind(&event.step_type)
            .bind(&event.status)
            .bind(&event.message)
            .bind(&data)
            .execute(&db)
            .await;
            if let Err(e) = res {
                tracing::warn!("failed to persist event to event_log: {e}");
            }
        });
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
