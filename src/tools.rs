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

/// Fetch public market data for a symbol (Yahoo Finance, no API key).
/// `symbol` examples: AAPL, MSFT, ^IXIC (NASDAQ Composite), NVDA.
pub async fn market_data(symbol: &str) -> Result<ToolOutput> {
    let sym = if symbol.trim().is_empty() { "^IXIC" } else { symbol.trim() };
    let url = format!("https://query1.finance.yahoo.com/v8/finance/chart/{sym}?interval=1d&range=5d");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "takoia-core")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("market data request for {sym} failed: {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await?;
    let result = &v["chart"]["result"][0];
    let meta = &result["meta"];
    let price = meta["regularMarketPrice"].as_f64().unwrap_or(0.0);
    let prev = meta["chartPreviousClose"].as_f64().or_else(|| meta["previousClose"].as_f64()).unwrap_or(price);
    let change = price - prev;
    let pct = if prev != 0.0 { change / prev * 100.0 } else { 0.0 };
    let cur = meta["currency"].as_str().unwrap_or("");
    let closes: Vec<String> = result["indicators"]["quote"][0]["close"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| format!("{x:.2}")).collect())
        .unwrap_or_default();
    let output = format!(
        "market_data {sym}: last {price:.2} {cur} ({change:+.2}, {pct:+.2}%), prev close {prev:.2}. Recent closes: {}.",
        closes.join(", ")
    );
    Ok(ToolOutput { output, usage: TokenUsage::default() })
}

/// Post a message to a Discord webhook (the agent's "alert" channel).
pub async fn send_discord(webhook_url: &str, content: &str) -> Result<()> {
    if webhook_url.trim().is_empty() {
        return Err(anyhow!("no discord webhook configured"));
    }
    let body = serde_json::json!({ "content": content.chars().take(1900).collect::<String>() });
    // Discord's Cloudflare rejects requests with no User-Agent (error 1010).
    let resp = reqwest::Client::new()
        .post(webhook_url)
        .header("User-Agent", "TakoIA-bot/1.0 (+https://takoia.szymkowiak.fr)")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("discord webhook returned {}", resp.status()));
    }
    Ok(())
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
