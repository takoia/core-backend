//! LLM abstraction: a single `LlmProvider` trait, an OpenAI-compatible client
//! that covers claude_max / ollama / gemini / codex, and a canned provider used
//! as an offline demo fallback so a run never breaks.

mod canned;
pub mod claude_cli;
pub mod oneshot;
mod openai_compat;
mod registry;

pub use canned::CannedProvider;
pub use claude_cli::{ClaudeCliProvider, TRANSPORT_SENTINEL as CLAUDE_CLI_SENTINEL};
pub use openai_compat::OpenAiCompatProvider;
pub use registry::ProviderRegistry;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Role of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// A single chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }
}

/// A completion request.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub messages: Vec<Message>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Ask the provider to perform live web search if it supports it
    /// (claude -p enables the WebSearch tool).
    pub enable_web_search: bool,
    /// Pure text-in/text-out: give the provider no tool at all (claude -p
    /// otherwise keeps its read-only file tool).
    pub no_tools: bool,
}

impl CompletionRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            model: None,
            temperature: None,
            max_tokens: None,
            enable_web_search: false,
            no_tools: false,
        }
    }

    /// Run this request without any tool.
    pub fn without_tools(mut self) -> Self {
        self.no_tools = true;
        self
    }

    /// Enable live web search for this request.
    pub fn with_web_search(mut self) -> Self {
        self.enable_web_search = true;
        self
    }
}

/// Token usage reported by the provider (basis for usage metering).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// A completion result.
#[derive(Debug, Clone)]
pub struct Completion {
    pub content: String,
    pub model: String,
    pub usage: TokenUsage,
}

/// Abstraction over an LLM backend. New providers plug in by implementing this.
// `async_trait` marks the boxed future `#[must_use]`, which clippy 1.99 flags.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider name (e.g. "claude_max", "ollama").
    fn name(&self) -> &str;

    /// Whether `enable_web_search` performs a real live search with this
    /// provider. Others would only imitate one, so callers must not ask.
    fn supports_web_search(&self) -> bool {
        false
    }

    /// Run a chat completion.
    async fn complete(&self, req: CompletionRequest) -> anyhow::Result<Completion>;
}
