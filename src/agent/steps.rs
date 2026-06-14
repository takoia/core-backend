//! Per-step defaults and prompt building. Each agent can override the system
//! prompt and options per step (the product differentiator); these are the
//! fallbacks used when an agent leaves a step unconfigured.

use crate::domain::StepType;

/// Default system prompt for a step, used when the agent has not customized it.
pub fn default_system_prompt(step: StepType, expertise: &str) -> String {
    let expert = if expertise.trim().is_empty() {
        String::new()
    } else {
        format!(" You are a domain expert in {expertise}.")
    };
    match step {
        StepType::Analyse => format!(
            "You are the ANALYSE step of an autonomous agent.{expert} Decompose the \
             objective into concrete sub-tasks and list the context needed. Use any \
             provided memory. Be concise and structured."
        ),
        StepType::Decision => format!(
            "You are the DECISION step of an autonomous agent.{expert} Given the \
             analysis, choose a clear plan and the tools to use (available tools: \
             web_search, market_data, send_discord, write_report). State the plan as \
             numbered steps and name the selected tool. Flag whether any action is \
             sensitive. Do not plan actions outside these tools."
        ),
        StepType::Action => format!(
            "You are the ACTION step of an autonomous agent.{expert} Execute the plan \
             using ONLY the tool results provided above. Synthesize what was actually \
             gathered into clear findings. IMPORTANT: the only real actions this \
             platform can perform are the tools whose results are given to you (e.g. \
             market_data, web_search) and sending a Discord alert. You CANNOT create \
             cron jobs, schedules, files, or config. Never invent identifiers, \
             schedules, or executed infrastructure, and never claim to have performed \
             an action that is not backed by a tool result above. State \
             recommendations as recommendations, not as completed actions."
        ),
        StepType::Restitution => format!(
            "You are the RESTITUTION step of an autonomous agent.{expert} Produce the \
             final deliverable as clean Markdown: a title, key findings, and a short \
             recommendation. This is what the user receives."
        ),
    }
}

/// Short human label for a step.
pub fn label(step: StepType) -> &'static str {
    match step {
        StepType::Analyse => "Analyse",
        StepType::Decision => "Decision",
        StepType::Action => "Action",
        StepType::Restitution => "Restitution",
    }
}
