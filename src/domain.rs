//! Core business types shared across the HTTP layer, the queue and the engine.

use serde::{Deserialize, Serialize};

/// The four explicit steps of the agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Analyse,
    Decision,
    Action,
    Restitution,
}

impl StepType {
    pub const ALL: [StepType; 4] = [
        StepType::Analyse,
        StepType::Decision,
        StepType::Action,
        StepType::Restitution,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            StepType::Analyse => "analyse",
            StepType::Decision => "decision",
            StepType::Action => "action",
            StepType::Restitution => "restitution",
        }
    }
}

/// How much autonomy the agent has before it must ask a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    /// The agent runs sensitive actions without asking.
    FullAuto,
    /// The agent pauses for human approval before a sensitive action.
    #[default]
    ConfirmBeforeAction,
}

impl AutonomyLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            AutonomyLevel::FullAuto => "full_auto",
            AutonomyLevel::ConfirmBeforeAction => "confirm_before_action",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "full_auto" => AutonomyLevel::FullAuto,
            _ => AutonomyLevel::ConfirmBeforeAction,
        }
    }
}

/// Lifecycle of a job in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    AwaitingApproval,
    Done,
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::AwaitingApproval => "awaiting_approval",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
        }
    }
}

/// Per-step configuration parsed from `agent_step_configs.options` (JSON).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepOptions {
    /// Provider name to use for this step (defaults to the agent/global default).
    #[serde(default)]
    pub provider: Option<String>,
    /// Model override for this step.
    #[serde(default)]
    pub model: Option<String>,
    /// Tools the agent is allowed to call during the Action step.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Per-tool parameters, e.g. { "symbol": "^IXIC", "discord_webhook": "https://..." }.
    #[serde(default)]
    pub tool_params: serde_json::Value,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
}
