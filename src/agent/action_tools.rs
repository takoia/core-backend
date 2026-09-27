//! Action-step tools: everything the agent may *do* besides talk. One place
//! dispatches them (market data, live web search, agent-to-agent calls on this
//! instance or a remote one) so the engine only sees a digest plus the LLM
//! usage to record.

use super::engine::{run_job, MemoryMode, RunOutcome};
use super::events::JobEvent;
use crate::domain::AutonomyLevel;
use crate::llm::{ProviderRegistry, TokenUsage};
use crate::queue::{self, ClaimedJob};
use crate::state::AppState;
use crate::tools;
use anyhow::Result;
use uuid::Uuid;

/// Bound on one remote agent invocation: a four-step run plus margin.
const A2A_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(4 * crate::llm::claude_cli::STEP_TIMEOUT.as_secs() + 60);

/// Everything the Action step's tools need from the run.
pub struct ToolRun<'a> {
    pub state: &'a AppState,
    pub job: &'a ClaimedJob,
    pub objective_prompt: &'a str,
    pub account_id: &'a str,
    pub registry: &'a ProviderRegistry,
    /// Provider configured for the Action step (None = account default).
    pub provider_name: Option<&'a str>,
    /// Tools the step config allows (`allowed_tools`).
    pub allowed: &'a [String],
    /// Per-tool parameters (`tool_params`).
    pub params: &'a serde_json::Value,
    /// Memory mode of the parent run; call_agent sub-runs inherit it.
    pub memory_mode: &'a MemoryMode,
}

/// Digest of every tool that ran, plus LLM usage to meter: (provider, model, usage).
#[derive(Default)]
pub struct Gathered {
    pub text: String,
    pub usage: Vec<(String, String, TokenUsage)>,
    /// A call_agent sub-run was served (at least partly) by the demo provider;
    /// the parent's deliverable is then built on fabricated data.
    pub canned: bool,
}

/// Run the allowed tools in a fixed order and gather their output. A tool that
/// fails reports its error into the digest so the agent can say so; only a
/// missing provider is a hard error (nothing sensible can run without one).
pub async fn gather(run: ToolRun<'_>) -> Result<Gathered> {
    let bus = &run.state.events;
    let job = run.job;
    let params = run.params;
    let mut out = Gathered::default();
    let has = |t: &str| run.allowed.iter().any(|a| a == t);

    if has("market_data") {
        let symbol = params
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("^IXIC");
        bus.publish(JobEvent::log(
            &job.id,
            format!("running tool: market_data ({symbol})"),
        ));
        match tools::market_data(symbol).await {
            Ok(o) => out.text.push_str(&format!("\n{}", o.output)),
            Err(e) => out.text.push_str(&format!("\nmarket_data error: {e}")),
        }
    }

    if has("web_search") {
        let provider = run.registry.resolve(run.provider_name)?;
        // Restrict to a specific site when the web_search tool has a `site` param.
        let site = params.get("site").and_then(|v| v.as_str()).unwrap_or("");
        let query = if site.trim().is_empty() {
            run.objective_prompt.to_string()
        } else {
            format!("{} site:{}", run.objective_prompt, site.trim())
        };
        if !provider.supports_web_search() {
            // Asking a model without live search to "search" yields fabricated
            // findings labelled as search results. Say so instead.
            let msg = format!(
                "web_search unavailable: provider '{}' has no live web search",
                provider.name()
            );
            bus.publish(JobEvent::log(&job.id, msg.clone()));
            out.text.push_str(&format!("\n{msg}."));
        } else {
            bus.publish(JobEvent::log(
                &job.id,
                format!(
                    "running tool: web_search{}",
                    if site.is_empty() {
                        String::new()
                    } else {
                        format!(" ({site})")
                    }
                ),
            ));
            match tools::execute(&provider, "web_search", &query).await {
                Ok(search) => {
                    out.usage
                        .push((provider.name().to_string(), search.model, search.usage));
                    out.text
                        .push_str(&format!("\nweb_search:\n{}", search.output));
                }
                Err(e) if run.registry.demo_mode() => {
                    tracing::warn!(error = %e, "web_search failed, canned fallback (demo mode)");
                    let search =
                        tools::execute(&run.registry.canned(), "web_search", &query).await?;
                    out.text
                        .push_str(&format!("\nweb_search (demo):\n{}", search.output));
                }
                Err(e) => out.text.push_str(&format!("\nweb_search error: {e}")),
            }
        }
    }

    // call_agent: this agent orchestrates other agents (multi-agent composition,
    // the substrate of the agent-to-agent flow). Each target runs synchronously,
    // read-only, and its deliverable is gathered for this agent to synthesize.
    if has("call_agent") {
        let targets = call_agent_targets(params);
        let depth = job_chain_depth(run.state, &job.id).await;
        if depth >= MAX_CALL_DEPTH {
            out.text
                .push_str("\ncall_agent skipped: max orchestration depth reached.");
        } else {
            for target in &targets {
                bus.publish(JobEvent::log(
                    &job.id,
                    format!("orchestrating: calling agent {target}"),
                ));
                match run_subagent(
                    run.state,
                    run.account_id,
                    &run.job.id,
                    target,
                    run.objective_prompt,
                    depth + 1,
                    run.memory_mode,
                )
                .await
                {
                    Ok((result, canned)) => {
                        out.canned |= canned;
                        out.text.push_str(&format!("\nagent[{target}]:\n{result}"));
                    }
                    Err(e) => out.text.push_str(&format!("\nagent[{target}] error: {e}")),
                }
            }
        }
    }

    // External agent-to-agent calls: invoke a published agent on this or another
    // TakoIA instance via its billed invoke API. The callee meters + bills the
    // call — the monetized agent-to-agent primitive.
    if let Some(arr) = params.get("a2a_calls").and_then(|v| v.as_array()) {
        for c in arr {
            let url = c.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                continue;
            }
            // Preferred: `key_connector` names an `a2a` connector holding the
            // consumer key. Inline `key` is honoured for existing agents.
            let key = match c.get("key_connector").and_then(|v| v.as_str()) {
                Some(name) => connector_secret(run.state, run.account_id, "a2a", name)
                    .await
                    .unwrap_or_default(),
                None => c
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            bus.publish(JobEvent::log(&job.id, format!("A2A call: {url}")));
            match run_a2a(url, &key, run.objective_prompt).await {
                Ok(r) => out.text.push_str(&format!("\na2a:\n{r}")),
                Err(e) => out.text.push_str(&format!("\na2a error: {e}")),
            }
        }
    }

    Ok(out)
}

/// Decrypt a connector secret referenced from a step config; a missing
/// connector is logged and yields `None` so the tool reports its own error.
pub(super) async fn connector_secret(
    state: &AppState,
    account_id: &str,
    kind: &str,
    name: &str,
) -> Option<String> {
    match crate::secrets::SecretManager::new(&state.cipher, &state.db)
        .connector_secret(account_id, kind, name)
        .await
    {
        Ok(Some(v)) => Some(v),
        Ok(None) => {
            tracing::warn!(
                kind,
                name,
                "step config references a connector that has no secret"
            );
            None
        }
        Err(e) => {
            tracing::warn!(kind, name, error = %e, "failed to resolve connector secret");
            None
        }
    }
}

// ── Multi-agent orchestration (call_agent) ──────────────────────────────────

/// Maximum agent-to-agent orchestration depth (anti-recursion guard).
const MAX_CALL_DEPTH: i64 = 3;

/// Target agent ids for the `call_agent` tool, read from the Action step params:
/// `{ "call_agents": ["id1","id2"] }` or `{ "call_agent_id": "id" }`.
fn call_agent_targets(params: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = params.get("call_agents").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }
    params
        .get("call_agent_id")
        .and_then(|v| v.as_str())
        .map(|s| vec![s.to_string()])
        .unwrap_or_default()
}

async fn job_chain_depth(state: &AppState, job_id: &str) -> i64 {
    sqlx::query_as::<_, (i64,)>("SELECT chain_depth FROM jobs WHERE id = ?")
        .bind(job_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|r| r.0)
        .unwrap_or(0)
}

/// Run another agent synchronously and return its deliverable (restitution).
/// The sub-run inherits the parent's memory mode: an owner's orchestration
/// teaches the callee, a consumer's stays inside that consumer's forks. `Box::pin` breaks the run_job -> call_agent -> run_job async cycle.
async fn run_subagent(
    state: &AppState,
    caller_account_id: &str,
    parent_job_id: &str,
    target_agent_id: &str,
    subtask: &str,
    depth: i64,
    mode: &MemoryMode,
) -> Result<(String, bool)> {
    // Same account only: another tenant's agent would run on their providers,
    // recall their memory and bill their usage on behalf of the caller.
    let target: Option<(String, String)> = sqlx::query_as(
        "SELECT account_id, autonomy_level FROM agents WHERE id = ? AND account_id = ?",
    )
    .bind(target_agent_id)
    .bind(caller_account_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((account_id, autonomy)) = target else {
        return Err(anyhow::anyhow!("target agent not found in this account"));
    };
    // A synchronous sub-run has nobody to approve an action: a
    // confirm-before-action target would park a job in awaiting_approval for
    // good. Refuse up front instead.
    if AutonomyLevel::from_db(&autonomy) != AutonomyLevel::FullAuto {
        return Err(anyhow::anyhow!(
            "target agent {target_agent_id} is '{}'; call_agent requires a '{}' target",
            autonomy,
            AutonomyLevel::FullAuto.as_str()
        ));
    }

    let objective_id = Uuid::new_v4().to_string();
    let sub_job_id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO objectives (id, account_id, agent_id, title, prompt) VALUES (?, ?, ?, 'sub-task', ?)",
    )
    .bind(&objective_id)
    .bind(&account_id)
    .bind(target_agent_id)
    .bind(subtask)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO jobs (id, objective_id, agent_id, status, synchronous, chain_depth, parent_job_id)
         VALUES (?, ?, ?, 'running', 1, ?, ?)",
    )
    .bind(&sub_job_id)
    .bind(&objective_id)
    .bind(target_agent_id)
    .bind(depth)
    .bind(parent_job_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let claimed = ClaimedJob {
        id: sub_job_id.clone(),
        objective_id,
        agent_id: target_agent_id.to_string(),
    };
    let canned = match Box::pin(run_job(state, &claimed, mode)).await {
        Ok(RunOutcome::Completed { canned }) => canned,
        Ok(RunOutcome::AwaitingApproval) => {
            let msg = "sub-agent paused for approval inside a synchronous call";
            queue::mark_failed(&state.db, &sub_job_id, msg).await.ok();
            return Err(anyhow::anyhow!(msg));
        }
        Err(e) => {
            // Synchronous jobs are never requeued by crash recovery, so an
            // unmarked failure would leave this row `running` forever.
            queue::mark_failed(&state.db, &sub_job_id, &format!("{e:#}"))
                .await
                .ok();
            return Err(e);
        }
    };

    let out: Option<(String,)> = sqlx::query_as(
        "SELECT output FROM steps WHERE job_id = ? AND step_type = 'restitution' ORDER BY position DESC LIMIT 1",
    )
    .bind(&sub_job_id)
    .fetch_optional(&state.db)
    .await?;
    let text = out
        .map(|(o,)| {
            serde_json::from_str::<serde_json::Value>(&o)
                .ok()
                .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
                .unwrap_or(o)
        })
        .unwrap_or_default();
    Ok((text, canned))
}

/// Call a published agent on this or another TakoIA instance through its billed
/// invoke API (Bearer consumer key). The callee meters tokens and bills the
/// call — the monetized agent-to-agent primitive. Returns the deliverable.
async fn run_a2a(url: &str, key: &str, input: &str) -> Result<String> {
    // SSRF guard: never let an agent point an A2A call at an internal address.
    let addrs = crate::net::validate_outbound_url(url).await?;
    // The peer runs a full synchronous agent (four steps, each bounded by its
    // own STEP_TIMEOUT), so the bound is minutes, not seconds.
    let resp = crate::net::pinned_client(url, &addrs, A2A_TIMEOUT)?
        .post(url)
        .header("Authorization", format!("Bearer {key}"))
        .json(&serde_json::json!({ "input": input }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "a2a call to {url} returned {}",
            resp.status()
        ));
    }
    let v: serde_json::Value = resp.json().await?;
    let out = v.get("output").and_then(|x| x.as_str()).unwrap_or_default();
    let cost = v.get("cost_usd").and_then(|x| x.as_f64()).unwrap_or(0.0);
    Ok(format!("{out}\n[billed via A2A: ${cost:.4}]"))
}
