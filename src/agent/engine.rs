//! The agent engine: run a job through the four explicit steps
//! analyse → decision → action → restitution, persisting each step, emitting
//! real-time events, reading/writing permanent memory, metering tokens, and
//! pausing for human approval when the agent is not fully autonomous.
//!
//! `run_job` is resumable: steps already `done` are reused, so a job that paused
//! for approval resumes without redoing earlier work.

use super::events::JobEvent;
use super::steps::{default_system_prompt, label};
use crate::domain::{AutonomyLevel, JobStatus, StepOptions, StepType};
use crate::llm::{CompletionRequest, Message, TokenUsage};
use crate::queue::{self, ClaimedJob};
use crate::state::AppState;
use crate::tools;
use anyhow::{Context, Result};
use std::collections::HashMap;
use uuid::Uuid;

/// Outcome of a run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    AwaitingApproval,
}

#[derive(sqlx::FromRow)]
struct ObjectiveRow {
    title: String,
    prompt: String,
    account_id: String,
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    autonomy_level: String,
    expertise_domain: String,
    persona: String,
}

#[derive(sqlx::FromRow)]
struct StepConfigRow {
    step_type: String,
    system_prompt: String,
    options: String,
}

/// Run (or resume) a job to completion or to an approval pause. When
/// `read_only_memory` is set (marketplace consumer invokes), the agent recalls
/// its memory but does NOT write to it, so the publisher's curated expertise is
/// never polluted by consumer inputs.
pub async fn run_job(state: &AppState, job: &ClaimedJob, read_only_memory: bool) -> Result<RunOutcome> {
    let bus = &state.events;
    bus.publish(JobEvent::status(&job.id, "running", "job started"));

    let objective = sqlx::query_as::<_, ObjectiveRow>(
        "SELECT title, prompt, account_id FROM objectives WHERE id = ?",
    )
    .bind(&job.objective_id)
    .fetch_one(&state.db)
    .await
    .context("objective not found")?;

    let agent = sqlx::query_as::<_, AgentRow>(
        "SELECT autonomy_level, expertise_domain, persona FROM agents WHERE id = ?",
    )
    .bind(&job.agent_id)
    .fetch_one(&state.db)
    .await
    .context("agent not found")?;
    let autonomy = AutonomyLevel::from_db(&agent.autonomy_level);

    let configs = load_step_configs(state, &job.agent_id).await?;
    let registry = state.load_registry(&objective.account_id).await?;
    let done = load_done_steps(state, &job.id).await?;

    // Permanent memory: recall accumulated expertise for prompt injection.
    let memory_ctx = state.memory.recall(&job.agent_id, &objective.prompt, 6).await;
    if !memory_ctx.trim().is_empty() {
        bus.publish(JobEvent::log(&job.id, "recalled expertise from memory"));
    }
    // Past corrections (learn from detected errors).
    let corrections = state.memory.recall_feedback(&job.agent_id, &objective.prompt, 5).await;
    if !corrections.trim().is_empty() {
        bus.publish(JobEvent::log(&job.id, "applying past corrections"));
    }

    let mut ctx = RunCtx {
        state,
        job,
        registry: &registry,
        configs: &configs,
        expertise: &agent.expertise_domain,
        persona: &agent.persona,
        account_id: &objective.account_id,
        session: Vec::new(),
        recalled_memory: memory_ctx.clone(),
        read_only: read_only_memory,
    };

    // ── Analyse ────────────────────────────────────────────────────────────
    let analyse_input = format!(
        "Objective: {}\n\n{}\n\nRelevant memory:\n{}\n\nPast corrections to apply:\n{}",
        objective.title,
        objective.prompt,
        if memory_ctx.trim().is_empty() { "(none)" } else { &memory_ctx },
        if corrections.trim().is_empty() { "(none)" } else { &corrections }
    );
    let analysis = ctx.step(StepType::Analyse, &analyse_input, &done, 0).await?;

    // ── Decision ───────────────────────────────────────────────────────────
    let decision_input = format!(
        "Objective: {}\n\nAnalysis:\n{}",
        objective.prompt, analysis
    );
    let decision = ctx.step(StepType::Decision, &decision_input, &done, 1).await?;

    // ── Approval gate (human-in-the-loop) ──────────────────────────────────
    if autonomy == AutonomyLevel::ConfirmBeforeAction {
        match latest_approval_status(state, &job.id).await? {
            Some(s) if s == "approved" => {
                bus.publish(JobEvent::log(&job.id, "action approved by human"));
            }
            Some(s) if s == "rejected" => {
                let msg = "action rejected by human";
                queue::mark_failed(&state.db, &job.id, msg).await?;
                bus.publish(JobEvent::status(&job.id, "failed", msg));
                return Ok(RunOutcome::Completed);
            }
            Some(_) => return Ok(RunOutcome::AwaitingApproval), // still pending
            None => {
                let approval_id = create_approval(state, &job.id, &decision).await?;
                queue::set_status(&state.db, &job.id, JobStatus::AwaitingApproval).await?;
                bus.publish(JobEvent::approval_required(
                    &job.id,
                    &approval_id,
                    "Approval required before the agent acts",
                ));
                return Ok(RunOutcome::AwaitingApproval);
            }
        }
    }

    // ── Action (tool execution via sandbox) ────────────────────────────────
    // Run every tool the agent's Action step allows (market_data, web_search…),
    // gathering their output. Extraction agents with no tools process the payload.
    let allowed = ctx.allowed_tools(StepType::Action);
    let params = ctx.tool_params(StepType::Action);
    if !allowed.is_empty() {
        bus.publish(JobEvent::step_started(&job.id, "action"));
    }
    let mut gathered = String::new();
    // Skip tool gathering when the Action step was already completed on a prior
    // attempt: the step result is reused below, so re-running the tools would
    // only waste calls and re-bill tokens (e.g. web_search).
    let action_done = done.contains_key(StepType::Action.as_str());
    if !action_done && allowed.iter().any(|t| t == "market_data") {
        let symbol = params.get("symbol").and_then(|v| v.as_str()).unwrap_or("^IXIC");
        bus.publish(JobEvent::log(&job.id, format!("running tool: market_data ({symbol})")));
        match tools::market_data(symbol).await {
            Ok(out) => gathered.push_str(&format!("\n{}", out.output)),
            Err(e) => gathered.push_str(&format!("\nmarket_data error: {e}")),
        }
    }
    if !action_done && allowed.iter().any(|t| t == "web_search") {
        let provider = registry.resolve(ctx.provider_for(StepType::Action).as_deref());
        // Restrict to a specific site when the web_search tool has a `site` param.
        let site = params.get("site").and_then(|v| v.as_str()).unwrap_or("");
        let query = if site.trim().is_empty() {
            objective.prompt.clone()
        } else {
            format!("{} site:{}", objective.prompt, site.trim())
        };
        bus.publish(JobEvent::log(&job.id, format!("running tool: web_search{}", if site.is_empty() { String::new() } else { format!(" ({site})") })));
        let search = match tools::execute(&provider, "web_search", &query).await {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!(error = %e, "web_search failed, using canned fallback");
                tools::execute(&registry.canned(), "web_search", &query).await?
            }
        };
        ctx.record_usage(&provider.name(), "web_search", search.usage).await;
        gathered.push_str(&format!("\nweb_search:\n{}", search.output));
    }
    // call_agent: this agent orchestrates other agents (multi-agent composition,
    // the substrate of the agent-to-agent flow). Each target runs synchronously,
    // read-only, and its deliverable is gathered for this agent to synthesize.
    if !action_done && allowed.iter().any(|t| t == "call_agent") {
        let targets = call_agent_targets(&params);
        let depth = job_chain_depth(state, &job.id).await;
        if depth >= MAX_CALL_DEPTH {
            gathered.push_str("\ncall_agent skipped: max orchestration depth reached.");
        } else {
            for target in &targets {
                bus.publish(JobEvent::log(&job.id, format!("orchestrating: calling agent {target}")));
                match run_subagent(state, target, &objective.prompt, depth + 1).await {
                    Ok(result) => gathered.push_str(&format!("\nagent[{target}]:\n{result}")),
                    Err(e) => gathered.push_str(&format!("\nagent[{target}] error: {e}")),
                }
            }
        }
    }
    let action_input = if gathered.trim().is_empty() {
        format!("Plan:\n{}\n\nInput to process:\n{}", decision, objective.prompt)
    } else {
        format!("Plan:\n{}\n\nGathered data:{}", decision, gathered)
    };
    let action = ctx
        .step_with_extra(StepType::Action, &action_input, &done, 2, TokenUsage::default())
        .await?;

    // ── Restitution (final deliverable + memory write) ─────────────────────
    // Whether Restitution was already completed on a prior attempt. On a resumed
    // run (approval requeue or crash recovery) `run_job` re-executes top to
    // bottom, so the post-restitution side effects below must not double-fire.
    let restitution_was_done = done.contains_key(StepType::Restitution.as_str());
    let restitution_input = format!(
        "Objective: {}\n\nFindings:\n{}",
        objective.prompt, action
    );
    let report = ctx.step(StepType::Restitution, &restitution_input, &done, 3).await?;

    // Persist what was learned so the agent gets more expert over time.
    // Read-only (marketplace) runs never write to the publisher's memory.
    // Skipped on a resumed-after-completion run so the summary is stored once.
    if !read_only_memory && !restitution_was_done {
        let summary = report.chars().take(600).collect::<String>();
        if let Err(e) = state
            .memory
            .store(&job.agent_id, "run-summary", &summary)
            .await
        {
            tracing::warn!(error = %e, "failed to persist memory");
        }
        // Capture HOW the user interacted (their wording/tone), separate from the
        // work, so the persona can later evolve from the relationship itself.
        let interaction = objective.prompt.chars().take(400).collect::<String>();
        let _ = state
            .memory
            .store(&job.agent_id, "interaction", &interaction)
            .await;
    }

    // Discord alert: if the agent uses send_discord and a webhook is set.
    // Skip on a resumed run whose Restitution was already done, so the alert is
    // never sent twice for the same job.
    if !restitution_was_done && allowed.iter().any(|t| t == "send_discord") {
        let webhook = params.get("discord_webhook").and_then(|v| v.as_str()).unwrap_or("");
        match tools::send_discord(webhook, &format!("**{}**\n{}", objective.title, report)).await {
            Ok(_) => bus.publish(JobEvent::log(&job.id, "alert sent to Discord")),
            Err(e) => bus.publish(JobEvent::log(&job.id, format!("discord notify failed: {e}"))),
        }
    }

    bus.publish(JobEvent::report(&job.id, &report));
    queue::set_status(&state.db, &job.id, JobStatus::Done).await?;
    bus.publish(JobEvent::status(&job.id, "done", "job completed"));
    // Skipped on a resumed-after-completion run so the run is counted once.
    if !restitution_was_done {
        increment_runs(state, &job.agent_id).await;
    }

    // Event choreography: emit this agent's events, triggering any wired agents.
    // Skipped on a resumed-after-completion run so downstream agents fire once.
    if !restitution_was_done {
        if let Err(e) = super::choreography::dispatch(state, &job.id, &job.agent_id, &report).await {
            tracing::warn!(error = %e, "choreography dispatch failed");
        }
    }

    Ok(RunOutcome::Completed)
}

/// Per-run context bundling everything the steps need.
struct RunCtx<'a> {
    state: &'a AppState,
    job: &'a ClaimedJob,
    registry: &'a crate::llm::ProviderRegistry,
    configs: &'a HashMap<String, StepConfigRow>,
    expertise: &'a str,
    persona: &'a str,
    account_id: &'a str,
    /// Accumulated outputs of THIS run's steps, injected into every later step
    /// so the agent keeps a continuous session memory across steps.
    session: Vec<String>,
    /// ICM recall computed once per run (the query — the objective prompt — is
    /// constant across steps).
    recalled_memory: String,
    /// Recall memory but never write it (marketplace consumer invokes).
    read_only: bool,
}

impl<'a> RunCtx<'a> {
    fn provider_for(&self, step: StepType) -> Option<String> {
        self.configs
            .get(step.as_str())
            .and_then(|c| serde_json::from_str::<StepOptions>(&c.options).ok())
            .and_then(|o| o.provider)
    }

    fn allowed_tools(&self, step: StepType) -> Vec<String> {
        self.configs
            .get(step.as_str())
            .and_then(|c| serde_json::from_str::<StepOptions>(&c.options).ok())
            .map(|o| o.allowed_tools)
            .unwrap_or_default()
    }

    fn tool_params(&self, step: StepType) -> serde_json::Value {
        self.configs
            .get(step.as_str())
            .and_then(|c| serde_json::from_str::<StepOptions>(&c.options).ok())
            .map(|o| o.tool_params)
            .unwrap_or(serde_json::Value::Null)
    }

    fn system_prompt(&self, step: StepType) -> String {
        match self.configs.get(step.as_str()) {
            Some(c) if !c.system_prompt.trim().is_empty() => c.system_prompt.clone(),
            _ => default_system_prompt(step, self.expertise),
        }
    }

    /// Run one LLM step (or reuse it if already done), persisting + emitting.
    async fn step(
        &mut self,
        step: StepType,
        input: &str,
        done: &HashMap<String, String>,
        position: i64,
    ) -> Result<String> {
        self.step_with_extra(step, input, done, position, TokenUsage::default())
            .await
    }

    async fn step_with_extra(
        &mut self,
        step: StepType,
        input: &str,
        done: &HashMap<String, String>,
        position: i64,
        _extra: TokenUsage,
    ) -> Result<String> {
        let bus = &self.state.events;
        if let Some(prev) = done.get(step.as_str()) {
            bus.publish(JobEvent::log(
                &self.job.id,
                format!("{} reused from previous attempt", label(step)),
            ));
            return Ok(prev.clone());
        }

        bus.publish(JobEvent::step_started(&self.job.id, step.as_str()));

        // ── ICM recall (computed once per run, reused here) ────────────────
        // Long-term memory (past runs + expertise) plus this run's earlier
        // steps, so the agent keeps a continuous, coherent session. The query
        // (objective prompt) is constant across steps, so the recall is done
        // once in `run_job` and cached on `RunCtx`.
        let recalled = self.recalled_memory.clone();
        if !recalled.trim().is_empty() {
            bus.publish(JobEvent::log(
                &self.job.id,
                format!("{}: recalled memory", label(step)),
            ));
        }

        let provider = self.registry.resolve(self.provider_for(step).as_deref());
        let mut messages = vec![Message::system(self.system_prompt(step))];
        // Per-agent persona (static identity/voice). The evolving half is the
        // ICM memory recalled just below — together they form the agent's
        // personalization that grows as memory consolidates.
        if !self.persona.trim().is_empty() {
            messages.push(Message::system(format!("Your persona / identity:\n{}", self.persona)));
        }
        // Tell the agent it owns a persistent ICM memory it recalls and writes to.
        messages.push(Message::system(
            "You have a persistent long-term memory (ICM). Before acting it is \
             recalled for you (the most important and most recent items first), \
             and your conclusions are saved back to it automatically after each \
             step. Build on what you already learned; do not contradict it and \
             avoid repeating it."
                .to_string(),
        ));
        if !self.session.is_empty() {
            messages.push(Message::system(format!(
                "Context from earlier steps of THIS run (stay consistent with it):\n{}",
                self.session.join("\n\n")
            )));
        }
        if !recalled.trim().is_empty() {
            messages.push(Message::system(format!(
                "Your recalled memory (most important & recent first):\n{recalled}"
            )));
        } else {
            messages.push(Message::system(
                "Your memory is currently empty for this objective — what you \
                 conclude now becomes your first memories."
                    .to_string(),
            ));
        }
        messages.push(Message::user(input.to_string()));
        let req = CompletionRequest::new(messages);

        let mut used_canned = false;
        let completion = match provider.complete(req.clone()).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(step = step.as_str(), error = %e, "step failed, canned fallback");
                bus.publish(JobEvent::log(
                    &self.job.id,
                    format!("{} provider error, using offline fallback", label(step)),
                ));
                used_canned = true;
                self.registry.canned().complete(req).await?
            }
        };

        self.record_usage(&provider.name(), step.as_str(), completion.usage)
            .await;
        persist_step(
            self.state,
            &self.job.id,
            step.as_str(),
            input,
            &completion.content,
            position,
        )
        .await?;

        // ── ICM store at the END of every step ─────────────────────────────
        // Persist this step's result so later steps and future runs recall it.
        // Skipped for read-only (marketplace) runs to protect the publisher, and
        // when the canned offline fallback was used (its generic demo content
        // would poison the agent's recalled memory).
        if !self.read_only && !used_canned {
            let trimmed: String = completion.content.chars().take(500).collect();
            if let Err(e) = self
                .state
                .memory
                .store(&self.job.agent_id, step.as_str(), &trimmed)
                .await
            {
                tracing::warn!(error = %e, "failed to store step memory");
            } else {
                self.state.events.publish(JobEvent::log(
                    &self.job.id,
                    format!("{}: saved to memory", label(step)),
                ));
            }
        }
        // Keep the full step output in the in-run session for the next steps.
        self.session.push(format!(
            "[{}]\n{}",
            step.as_str(),
            completion.content.chars().take(800).collect::<String>()
        ));

        self.state.events.publish(JobEvent::step_completed(
            &self.job.id,
            step.as_str(),
            serde_json::json!({ "text": completion.content }),
        ));
        Ok(completion.content)
    }

    async fn record_usage(&self, provider: &str, model: &str, usage: TokenUsage) {
        let cost = notional_cost(usage);
        let res = sqlx::query(
            r#"INSERT INTO token_usage
               (id, account_id, agent_id, job_id, provider, model,
                prompt_tokens, completion_tokens, estimated_cost)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(self.account_id)
        .bind(&self.job.agent_id)
        .bind(&self.job.id)
        .bind(provider)
        .bind(model)
        .bind(usage.prompt_tokens as i64)
        .bind(usage.completion_tokens as i64)
        .bind(cost)
        .execute(&self.state.db)
        .await;
        if let Err(e) = res {
            tracing::warn!(error = %e, "failed to record token usage");
        }
        self.state.events.publish(JobEvent::log(
            &self.job.id,
            format!(
                "{provider} used {} prompt + {} completion tokens",
                usage.prompt_tokens, usage.completion_tokens
            ),
        ));
    }
}

/// Notional USD cost estimate (real cost is the flat plan). Indicative only,
/// the basis for usage-based billing of marketplace consumers.
fn notional_cost(usage: TokenUsage) -> f64 {
    (usage.prompt_tokens as f64 * 3.0 + usage.completion_tokens as f64 * 15.0) / 1_000_000.0
}

async fn load_step_configs(
    state: &AppState,
    agent_id: &str,
) -> Result<HashMap<String, StepConfigRow>> {
    let rows = sqlx::query_as::<_, StepConfigRow>(
        "SELECT step_type, system_prompt, options FROM agent_step_configs WHERE agent_id = ?",
    )
    .bind(agent_id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.step_type.clone(), r)).collect())
}

async fn load_done_steps(state: &AppState, job_id: &str) -> Result<HashMap<String, String>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        step_type: String,
        output: String,
    }
    let rows = sqlx::query_as::<_, Row>(
        "SELECT step_type, output FROM steps WHERE job_id = ? AND status = 'done'",
    )
    .bind(job_id)
    .fetch_all(&state.db)
    .await?;
    let mut map = HashMap::new();
    for r in rows {
        let text = serde_json::from_str::<serde_json::Value>(&r.output)
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
            .unwrap_or(r.output);
        map.insert(r.step_type, text);
    }
    Ok(map)
}

async fn persist_step(
    state: &AppState,
    job_id: &str,
    step_type: &str,
    input: &str,
    output: &str,
    position: i64,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO steps
           (id, job_id, step_type, status, input, output, position, started_at, finished_at)
           VALUES (?, ?, ?, 'done', ?, ?, ?,
                   strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                   strftime('%Y-%m-%dT%H:%M:%fZ','now'))"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(job_id)
    .bind(step_type)
    .bind(serde_json::json!({ "text": input }).to_string())
    .bind(serde_json::json!({ "text": output }).to_string())
    .bind(position)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn latest_approval_status(state: &AppState, job_id: &str) -> Result<Option<String>> {
    let status: Option<(String,)> = sqlx::query_as(
        "SELECT status FROM approvals WHERE job_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(status.map(|s| s.0))
}

async fn create_approval(state: &AppState, job_id: &str, decision: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO approvals (id, job_id, status, summary, payload)
           VALUES (?, ?, 'pending', ?, ?)"#,
    )
    .bind(&id)
    .bind(job_id)
    .bind("The agent plans to act. Review and approve to continue.")
    .bind(serde_json::json!({ "plan": decision }).to_string())
    .execute(&state.db)
    .await?;
    Ok(id)
}

async fn increment_runs(state: &AppState, agent_id: &str) {
    let _ = sqlx::query("UPDATE agents SET runs_count = runs_count + 1 WHERE id = ?")
        .bind(agent_id)
        .execute(&state.db)
        .await;
}

/// Build the engine error into a job failure (used by the worker).
pub async fn fail(state: &AppState, job_id: &str, err: &anyhow::Error) {
    let msg = format!("{err:#}");
    let _ = queue::mark_failed(&state.db, job_id, &msg).await;
    state
        .events
        .publish(JobEvent::status(job_id, "failed", msg));
}

// ── Multi-agent orchestration (call_agent) ──────────────────────────────────

/// Maximum agent-to-agent orchestration depth (anti-recursion guard).
const MAX_CALL_DEPTH: i64 = 3;

/// Target agent ids for the `call_agent` tool, read from the Action step params:
/// `{ "call_agents": ["id1","id2"] }` or `{ "call_agent_id": "id" }`.
fn call_agent_targets(params: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = params.get("call_agents").and_then(|v| v.as_array()) {
        return arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
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
/// Read-only so the callee's curated memory is never polluted by the caller's
/// sub-task. `Box::pin` breaks the run_job -> call_agent -> run_job async cycle.
async fn run_subagent(
    state: &AppState,
    target_agent_id: &str,
    subtask: &str,
    depth: i64,
) -> Result<String> {
    let account: Option<(String,)> = sqlx::query_as("SELECT account_id FROM agents WHERE id = ?")
        .bind(target_agent_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((account_id,)) = account else {
        return Err(anyhow::anyhow!("target agent not found"));
    };

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
        "INSERT INTO jobs (id, objective_id, agent_id, status, synchronous, chain_depth) VALUES (?, ?, ?, 'running', 1, ?)",
    )
    .bind(&sub_job_id)
    .bind(&objective_id)
    .bind(target_agent_id)
    .bind(depth)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let claimed = ClaimedJob {
        id: sub_job_id.clone(),
        objective_id,
        agent_id: target_agent_id.to_string(),
    };
    Box::pin(run_job(state, &claimed, true)).await?;

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
    Ok(text)
}
