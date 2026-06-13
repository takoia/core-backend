//! OpenAI-compatible chat completions client.
//!
//! Works for any provider exposing `POST {base_url}/chat/completions`:
//! claude_max (plan proxy), ollama (local), gemini (OpenAI-compat endpoint),
//! codex / OpenAI.

use super::{Completion, CompletionRequest, LlmProvider, Message, Role, TokenUsage};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A configured OpenAI-compatible provider.
pub struct OpenAiCompatProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    default_model: String,
    client: reqwest::Client,
}

impl OpenAiCompatProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        default_model: impl Into<String>,
    ) -> Self {
        // Trim a trailing slash so we can join paths cleanly.
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            name: name.into(),
            base_url,
            api_key,
            default_model: default_model.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: CompletionRequest) -> anyhow::Result<Completion> {
        let model = req
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| self.default_model.clone());
        if model.is_empty() {
            return Err(anyhow!("no model configured for provider '{}'", self.name));
        }

        let body = ChatRequest {
            model: &model,
            messages: req.messages.iter().map(WireMessage::from).collect(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: false,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let mut builder = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        let resp = builder
            .send()
            .await
            .with_context(|| format!("request to provider '{}' failed", self.name))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "provider '{}' returned {}: {}",
                self.name,
                status,
                text.chars().take(500).collect::<String>()
            ));
        }

        let parsed: ChatResponse = serde_json::from_str(&text)
            .with_context(|| format!("invalid response from provider '{}'", self.name))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("provider '{}' returned no choices", self.name))?;

        let usage = parsed.usage.unwrap_or_default();
        Ok(Completion {
            content: choice.message.content.unwrap_or_default(),
            model,
            usage: TokenUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
            },
        })
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

impl<'a> From<&'a Message> for WireMessage<'a> {
    fn from(m: &'a Message) -> Self {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        WireMessage { role, content: &m.content }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}
