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
    /// Store this step's own output in long-term memory. Off by default:
    /// intermediate reasoning (analysis, plan, tool digest) is not knowledge
    /// and only dilutes recall. The run summary and the user's interaction are
    /// always stored at the end of the run.
    #[serde(default)]
    pub remember: bool,
    /// Per-tool parameters, e.g. { "symbol": "^IXIC", "discord_webhook": "https://..." }.
    #[serde(default)]
    pub tool_params: serde_json::Value,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
}

/// Tool parameter keys that hold credentials when set inline. The supported
/// way is a connector reference (`discord_connector`, `a2a_calls[].key_connector`);
/// inline values are still honoured for existing agents but never leave the
/// server unmasked.
const INLINE_SECRET_KEYS: [&str; 1] = ["discord_webhook"];

/// Mask (or strip) inline credentials inside a step's `options` JSON so they can
/// be shown to non-owners or exported. `strip` removes the keys; otherwise they
/// are replaced by `"***"`. Non-object input is returned untouched.
pub fn redact_step_options(options: &mut serde_json::Value, strip: bool) {
    let Some(tp) = options
        .get_mut("tool_params")
        .and_then(|v| v.as_object_mut())
    else {
        return;
    };
    for key in INLINE_SECRET_KEYS {
        if tp.contains_key(key) {
            if strip {
                tp.remove(key);
            } else {
                tp.insert(key.to_string(), serde_json::Value::String("***".into()));
            }
        }
    }
    if let Some(calls) = tp.get_mut("a2a_calls").and_then(|v| v.as_array_mut()) {
        for call in calls.iter_mut().filter_map(|c| c.as_object_mut()) {
            if call.contains_key("key") {
                if strip {
                    call.remove("key");
                } else {
                    call.insert("key".into(), serde_json::Value::String("***".into()));
                }
            }
        }
    }
}

/// Tool parameter keys that carry, or route, credentials: the inline secrets,
/// the connector references, and the A2A call list (its URLs decide where a
/// stored key is sent). Only an owner may change them.
const CREDENTIAL_PARAMS: [&str; 3] = ["discord_webhook", "discord_connector", "a2a_calls"];

/// For a write by a non-owner: keep the stored credential-bearing parameters
/// exactly as they are, whatever `incoming` says. Restoring a masked value by
/// position was not enough — an editor could keep the `"***"` mask and change
/// the URL next to it, sending the real key to a host of their choosing.
pub fn preserve_credential_params(incoming: &mut serde_json::Value, current: &serde_json::Value) {
    let cur_tp = current.get("tool_params").and_then(|v| v.as_object());
    let Some(obj) = incoming.as_object_mut() else {
        return;
    };
    let tp = obj
        .entry("tool_params")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    let Some(tp) = tp.as_object_mut() else {
        return;
    };
    for key in CREDENTIAL_PARAMS {
        match cur_tp.and_then(|c| c.get(key)).cloned() {
            Some(v) => {
                tp.insert(key.to_string(), v);
            }
            None => {
                tp.remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn non_owner_writes_cannot_move_or_add_credentials() {
        let current = json!({ "tool_params": { "discord_webhook": "https://real", "a2a_calls": [{ "url": "https://peer", "key": "real-key" }] } });
        // Editor keeps the mask but redirects the URL: the stored call list wins.
        let mut incoming = json!({ "tool_params": { "discord_webhook": "***", "symbol": "AAPL", "a2a_calls": [{ "url": "https://attacker", "key": "***" }] } });
        preserve_credential_params(&mut incoming, &current);
        assert_eq!(incoming["tool_params"]["discord_webhook"], "https://real");
        assert_eq!(
            incoming["tool_params"]["a2a_calls"][0]["url"],
            "https://peer"
        );
        assert_eq!(incoming["tool_params"]["a2a_calls"][0]["key"], "real-key");
        assert_eq!(
            incoming["tool_params"]["symbol"], "AAPL",
            "non-credential params are theirs"
        );
        // Editor tries to add a connector reference or a call list: dropped.
        let mut added = json!({ "tool_params": { "discord_connector": "alerts", "a2a_calls": [{ "url": "https://attacker", "key_connector": "prod" }] } });
        preserve_credential_params(&mut added, &json!({}));
        assert!(added["tool_params"].get("discord_connector").is_none());
        assert!(added["tool_params"].get("a2a_calls").is_none());
        // Options without tool_params get the stored credentials back, nothing else changes.
        let mut bare = json!({ "provider": "x" });
        preserve_credential_params(&mut bare, &current);
        assert_eq!(bare["provider"], "x");
        assert_eq!(bare["tool_params"]["discord_webhook"], "https://real");
    }

    #[test]
    fn masks_inline_credentials_and_keeps_the_rest() {
        let mut v = json!({
            "allowed_tools": ["send_discord"],
            "tool_params": {
                "symbol": "^IXIC",
                "discord_webhook": "https://discord.com/api/webhooks/1/abc",
                "a2a_calls": [{ "url": "https://peer/api/v1/agents/x/invoke", "key": "sk_takoia_1" }]
            }
        });
        redact_step_options(&mut v, false);
        assert_eq!(v["tool_params"]["symbol"], "^IXIC");
        assert_eq!(v["tool_params"]["discord_webhook"], "***");
        assert_eq!(v["tool_params"]["a2a_calls"][0]["key"], "***");
        assert_eq!(
            v["tool_params"]["a2a_calls"][0]["url"],
            "https://peer/api/v1/agents/x/invoke"
        );
    }

    #[test]
    fn strips_inline_credentials_for_export() {
        let mut v = json!({ "tool_params": { "discord_webhook": "x", "a2a_calls": [{ "url": "u", "key": "k" }] } });
        redact_step_options(&mut v, true);
        assert!(v["tool_params"].get("discord_webhook").is_none());
        assert!(v["tool_params"]["a2a_calls"][0].get("key").is_none());
        assert_eq!(v["tool_params"]["a2a_calls"][0]["url"], "u");
    }

    #[test]
    fn leaves_options_without_tool_params_alone() {
        let mut v = json!({ "provider": "claude_max" });
        let before = v.clone();
        redact_step_options(&mut v, false);
        assert_eq!(v, before);
        let mut s = json!("not an object");
        redact_step_options(&mut s, true);
        assert_eq!(s, json!("not an object"));
    }
}
