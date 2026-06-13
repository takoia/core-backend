//! Canned provider: deterministic offline responses used as a demo fallback so
//! a run never breaks when no real provider is reachable. It inspects the prompt
//! for step keywords and returns plausible, coherent content for each of the
//! four steps.

use super::{Completion, CompletionRequest, LlmProvider, Role, TokenUsage};
use async_trait::async_trait;

/// A provider that returns fixed, plausible content without any network call.
pub struct CannedProvider {
    name: String,
}

impl CannedProvider {
    pub fn new() -> Self {
        Self { name: "canned".to_string() }
    }
}

impl Default for CannedProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for CannedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: CompletionRequest) -> anyhow::Result<Completion> {
        let haystack = req
            .messages
            .iter()
            .map(|m| m.content.to_lowercase())
            .collect::<Vec<_>>()
            .join("\n");

        // The user's actual objective is the last user message.
        let objective = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let content = if haystack.contains("restitution") || haystack.contains("final report") {
            canned_report(&objective)
        } else if haystack.contains("decision") || haystack.contains("choose a plan") {
            canned_decision()
        } else if haystack.contains("action") || haystack.contains("execute the plan") {
            canned_action()
        } else {
            canned_analysis(&objective)
        };

        let usage = TokenUsage {
            prompt_tokens: (haystack.len() / 4) as u32,
            completion_tokens: (content.len() / 4) as u32,
        };
        Ok(Completion { content, model: "canned-demo".to_string(), usage })
    }
}

fn canned_analysis(objective: &str) -> String {
    format!(
        "Analysis of the objective:\n\
         - Goal: {objective}\n\
         - Sub-tasks: identify recent sources, extract key announcements, group by theme.\n\
         - Axes to watch: new agent frameworks, autonomy levels, notable product launches.\n\
         - Context from memory: none relevant yet.",
        objective = objective.trim()
    )
}

fn canned_decision() -> String {
    "Plan:\n\
     1. Run a web search for recent AI agent announcements.\n\
     2. Summarize the most relevant items.\n\
     3. Produce a synthetic weekly report.\n\
     Selected tool: web_search. This action is not sensitive."
        .to_string()
}

fn canned_action() -> String {
    "web_search results (demo):\n\
     - New autonomous agent framework released with adjustable autonomy levels.\n\
     - Major provider ships native tool-calling improvements.\n\
     - Open-source project adds human-in-the-loop approval gates.\n\
     - Industry report: usage-based billing becomes standard for agent platforms."
        .to_string()
}

fn canned_report(objective: &str) -> String {
    format!(
        "# Weekly AI Agents Watch\n\n\
         **Objective:** {objective}\n\n\
         ## Highlights\n\
         - A new autonomous agent framework introduces adjustable autonomy levels.\n\
         - Native tool-calling keeps improving across major providers.\n\
         - Human-in-the-loop approval gates are becoming a standard safety pattern.\n\n\
         ## Market signal\n\
         Usage-based billing is emerging as the default model for agent platforms.\n\n\
         ## Recommendation\n\
         Prioritize customizable agent steps and a clear human approval flow.\n",
        objective = objective.trim()
    )
}
