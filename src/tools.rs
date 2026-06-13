//! Concrete tools the agent can execute during the Action step, run through a
//! minimal sandbox boundary (`ToolExecutor`). Tools that need reasoning or live
//! data delegate to the active LLM provider, so `claude -p` performs real web
//! search while the canned provider yields demo results offline.

use crate::llm::{CompletionRequest, LlmProvider, Message, TokenUsage};
use anyhow::{anyhow, Result};
use std::sync::Arc;

/// Result of running a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub output: String,
    pub usage: TokenUsage,
}

/// Names of the tools the platform knows about.
pub const KNOWN_TOOLS: &[&str] = &["web_search", "write_report"];

/// Whether a tool is considered sensitive (gated by human approval when the
/// agent is not fully autonomous).
pub fn is_sensitive(tool: &str) -> bool {
    matches!(tool, "send_discord" | "write_report")
}

/// Execute a tool by name.
pub async fn execute(
    provider: &Arc<dyn LlmProvider>,
    tool: &str,
    input: &str,
) -> Result<ToolOutput> {
    match tool {
        "web_search" => web_search(provider, input).await,
        "write_report" => Ok(ToolOutput {
            output: input.to_string(),
            usage: TokenUsage::default(),
        }),
        other => Err(anyhow!("unknown tool: {other}")),
    }
}

/// Search the web for recent information about `query` and return a concise,
/// bulleted digest. Real search when the provider is `claude -p`.
async fn web_search(provider: &Arc<dyn LlmProvider>, query: &str) -> Result<ToolOutput> {
    let req = CompletionRequest::new(vec![
        Message::system(
            "You are a web research tool. Search the web for recent, factual \
             information and return a concise bulleted digest (max 8 bullets). \
             Each bullet: one specific finding with the source name if known. \
             No preamble, no conclusion.",
        ),
        Message::user(format!("web_search query: {query}")),
    ])
    .with_web_search();
    let completion = provider.complete(req).await?;
    Ok(ToolOutput {
        output: completion.content,
        usage: completion.usage,
    })
}
