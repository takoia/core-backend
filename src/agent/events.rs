//! Real-time job events broadcast to SSE subscribers (and later Discord).

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
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<JobEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.tx.subscribe()
    }

    /// Publish an event. Ignores the "no subscribers" case.
    pub fn publish(&self, event: JobEvent) {
        let _ = self.tx.send(event);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
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
