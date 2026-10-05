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
use crate::memory::MemoryScope;
use crate::queue::{self, ClaimedJob};
use crate::state::AppState;
use crate::tools;
use anyhow::{Context, Result};
use std::collections::HashMap;
use uuid::Uuid;

/// Outcome of a run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The run finished. `canned` is true when at least one step was served by
    /// the offline demo provider (only possible in demo mode); such output
    /// must never be billed or presented as real.
    Completed {
        canned: bool,
    },
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

/// Whose memory a run reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryMode {
    /// The publisher's own run: recall and write the agent's memory; the
    /// agent's inner life reacts to the interaction.
    Owner,
    /// A marketplace consumer: recall the knowledge distilled from the
    /// publisher's memory (read-only, never its raw episodes) plus the
    /// consumer's own fork, and write only to that fork. The agent keeps
    /// learning from the person using it without touching the expertise they
    /// pay for.
    Consumer { account_id: String },
}

impl MemoryMode {
    /// Scope that receives this run's writes.
    fn write_scope(&self, agent_id: &str) -> MemoryScope {
        match self {
            MemoryMode::Owner => MemoryScope::owner(agent_id),
            MemoryMode::Consumer { account_id } => MemoryScope::consumer(agent_id, account_id),
        }
    }

    /// Consumer account whose fork is recalled after the agent's knowledge.
    fn recall_consumer(&self) -> Option<&str> {
        match self {
            MemoryMode::Consumer { account_id } => Some(account_id),
            MemoryMode::Owner => None,
        }
    }
}

/// Run (or resume) a job to completion or to an approval pause. `mode` decides
/// whose memory is recalled and where this run's learnings are written (see
/// [`MemoryMode`]).
pub async fn run_job(state: &AppState, job: &ClaimedJob, mode: &MemoryMode) -> Result<RunOutcome> {
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
    let registry = state
        .load_registry(&objective.account_id, &job.agent_id)
        .await?;
    let done = load_done_steps(state, &job.id).await?;

    // Permanent memory: recall accumulated expertise for prompt injection.
    let memory_ctx = state
        .memory
        .recall_composed(&job.agent_id, mode.recall_consumer(), &objective.prompt, 6)
        .await;
    if !memory_ctx.trim().is_empty() {
        bus.publish(JobEvent::log(&job.id, "recalled expertise from memory"));
    }
    // Past corrections (learn from detected errors): the `correction` episodes
    // of the scope this run writes to, which the personal block above leaves
    // out. The publisher's raw corrections quote the publisher's own jobs and
    // reach a consumer run only once distilled into knowledge.
    let corrections = state
        .memory
        .recall_feedback(&mode.write_scope(&job.agent_id), &objective.prompt, 5)
        .await;
    if !corrections.trim().is_empty() {
        bus.publish(JobEvent::log(&job.id, "applying past corrections"));
    }

    // Inner life: the agent's mood/emotions + personality colour this run's tone,
    // gated by the agent's personalization toggles.
    let inner = super::inner_life::current(&state.db, &job.agent_id).await;
    let pers = super::inner_life::personalization(&state.db, &job.agent_id).await;
    let mood_flavor = super::inner_life::flavor_line(&inner, &pers);

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
        mood_flavor,
        write_scope: mode.write_scope(&job.agent_id),
        last_step_canned: false,
        any_step_canned: false,
        max_tokens: match mode {
            MemoryMode::Consumer { .. } => Some(state.config.invoke_max_output_tokens),
            MemoryMode::Owner => None,
        },
    };

    // ── Analyse ────────────────────────────────────────────────────────────
    // The recalled memory is injected once per step by `RunCtx::step` (system
    // message); repeating it here doubled the Analyse prompt.
    let analyse_input = format!(
        "Objective: {}\n\n{}\n\nPast corrections to apply:\n{}",
        objective.title,
        objective.prompt,
        if corrections.trim().is_empty() {
            "(none)"
        } else {
            &corrections
        }
    );
    let analysis = ctx
        .step(StepType::Analyse, &analyse_input, &done, 0)
        .await?;

    // ── Decision ───────────────────────────────────────────────────────────
    let decision_input = format!("Objective: {}\n\nAnalysis:\n{}", objective.prompt, analysis);
    let decision = ctx
        .step(StepType::Decision, &decision_input, &done, 1)
        .await?;

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
                return Ok(RunOutcome::Completed { canned: false });
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

    // ── Action (tool execution) ────────────────────────────────────────────
    // Every tool the Action step allows runs through `action_tools::gather`;
    // its digest is what the Action step synthesises. Skipped when the Action
    // step was already completed on a prior attempt (the step result is reused
    // below, so re-running the tools would only waste calls and re-bill).
    let allowed = ctx.allowed_tools(StepType::Action);
    let params = ctx.tool_params(StepType::Action);
    if !allowed.is_empty() {
        bus.publish(JobEvent::step_started(&job.id, "action"));
    }
    let action_done = done.contains_key(StepType::Action.as_str());
    let mut gathered = String::new();
    if !action_done {
        let provider_name = ctx.provider_for(StepType::Action);
        let out = super::action_tools::gather(super::action_tools::ToolRun {
            state,
            job,
            objective_prompt: &objective.prompt,
            account_id: &objective.account_id,
            registry: &registry,
            provider_name: provider_name.as_deref(),
            allowed: &allowed,
            params: &params,
            memory_mode: mode,
        })
        .await?;
        for (provider, model, usage) in out.usage {
            ctx.record_usage(&provider, &model, usage).await;
        }
        ctx.any_step_canned |= out.canned;
        gathered = out.text;
    }
    let action_input = if gathered.trim().is_empty() {
        format!(
            "Plan:\n{}\n\nInput to process:\n{}",
            decision, objective.prompt
        )
    } else {
        format!("Plan:\n{}\n\nGathered data:{}", decision, gathered)
    };
    let action = ctx
        .step_with_extra(
            StepType::Action,
            &action_input,
            &done,
            2,
            TokenUsage::default(),
        )
        .await?;

    // ── Restitution (final deliverable + memory write) ─────────────────────
    // Whether Restitution was already completed on a prior attempt. On a resumed
    // run (approval requeue or crash recovery) `run_job` re-executes top to
    // bottom, so the post-restitution side effects below must not double-fire.
    let restitution_was_done = done.contains_key(StepType::Restitution.as_str());
    let restitution_input = format!("Objective: {}\n\nFindings:\n{}", objective.prompt, action);
    let report = ctx
        .step(StepType::Restitution, &restitution_input, &done, 3)
        .await?;

    // Persist what was learned so the agent gets more expert over time.
    // Read-only (marketplace) runs never write to the publisher's memory.
    // Skipped on a resumed-after-completion run so the summary is stored once.
    if !restitution_was_done {
        let scope = mode.write_scope(&job.agent_id);
        // Provenance: a consumer run is their data, kept under the marketplace
        // contract; an owner run is the publisher's own.
        let prov = crate::memory::Provenance::for_run(&scope, &job.id);
        let summary = report.chars().take(600).collect::<String>();
        if let Err(e) = state
            .memory
            .store_with(&scope, "run-summary", &summary, &prov)
            .await
        {
            tracing::warn!(error = %e, "failed to persist memory");
        }
        // Capture HOW the user interacted (their wording/tone), separate from the
        // work, so the persona can later evolve from the relationship itself.
        let interaction = objective.prompt.chars().take(400).collect::<String>();
        let _ = state
            .memory
            .store_with(&scope, "interaction", &interaction, &prov)
            .await;
        // Inner life: a completed interaction grows familiarity and lifts energy.
        // Only the owner's own interactions move the agent's mood/familiarity;
        // a consumer's call must not shift the publisher's agent.
        if *mode == MemoryMode::Owner {
            super::inner_life::note_activity(&state.db, &job.agent_id).await;
        }
    }

    // Discord alert: only when the agent actually produced an alert. The agent
    // emits `NO_ALERT` (anywhere in the restitution) when there is nothing
    // actionable, so a routine "no signal" run stays silent instead of spamming.
    // Skip on a resumed run whose Restitution was already done (never twice).
    let suppressed = ctx.last_step_canned || report.to_ascii_uppercase().contains("NO_ALERT");
    if !restitution_was_done && allowed.iter().any(|t| t == "send_discord") {
        if suppressed {
            bus.publish(JobEvent::log(
                &job.id,
                "no alert (agent reported no actionable signal)",
            ));
        } else {
            // Preferred: `discord_connector` names a `discord` connector whose
            // secret is the webhook URL. Inline `discord_webhook` still works.
            let webhook = match params.get("discord_connector").and_then(|v| v.as_str()) {
                Some(name) => super::action_tools::connector_secret(
                    state,
                    &objective.account_id,
                    "discord",
                    name,
                )
                .await
                .unwrap_or_default(),
                None => params
                    .get("discord_webhook")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            match tools::send_discord(&webhook, &format!("**{}**\n{}", objective.title, report))
                .await
            {
                Ok(_) => bus.publish(JobEvent::log(&job.id, "alert sent to Discord")),
                Err(e) => bus.publish(JobEvent::log(
                    &job.id,
                    format!("discord notify failed: {e}"),
                )),
            }
        }
    }

    bus.publish(JobEvent::report(&job.id, &report));
    if !queue::finish(&state.db, &job.id).await? {
        // Failed underneath us (stale sweep): report it, do not undo it.
        bus.publish(JobEvent::status(
            &job.id,
            "failed",
            "job was marked failed while running",
        ));
        return Err(anyhow::anyhow!(
            "job {} was marked failed while running",
            job.id
        ));
    }
    bus.publish(JobEvent::status(&job.id, "done", "job completed"));
    // Skipped on a resumed-after-completion run so the run is counted once.
    if !restitution_was_done {
        increment_runs(state, &job.agent_id).await;
    }

    // Event choreography: emit this agent's events, triggering any wired agents.
    // Skipped on a resumed-after-completion run so downstream agents fire once.
    if !restitution_was_done {
        if let Err(e) = super::choreography::dispatch(state, &job.id, &job.agent_id, &report).await
        {
            tracing::warn!(error = %e, "choreography dispatch failed");
        }
    }

    Ok(RunOutcome::Completed {
        canned: ctx.any_step_canned,
    })
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
    /// One line describing the agent's current mood/energy, injected so its tone
    /// reflects how it feels right now (its "inner life").
    mood_flavor: String,
    /// Where this run's step memories go (owner memory or the consumer's fork).
    write_scope: MemoryScope,
    /// Whether the most recent step fell back to the canned offline provider
    /// (its generic demo content must not be pushed as a real Discord alert).
    last_step_canned: bool,
    /// Whether ANY step of this run used the canned provider (demo mode only).
    any_step_canned: bool,
    /// Per-step output budget (marketplace consumer runs); None = provider default.
    max_tokens: Option<u32>,
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

    fn remember(&self, step: StepType) -> bool {
        self.configs
            .get(step.as_str())
            .and_then(|c| serde_json::from_str::<StepOptions>(&c.options).ok())
            .map(|o| o.remember)
            .unwrap_or(false)
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

        let provider = self.registry.resolve(self.provider_for(step).as_deref())?;
        let mut messages = vec![Message::system(self.system_prompt(step))];
        // Per-agent persona (static identity/voice). The evolving half is the
        // memory recalled just below — together they form the agent's
        // personalization, which grows as episodes are distilled into knowledge.
        if !self.persona.trim().is_empty() {
            messages.push(Message::system(format!(
                "Your persona / identity:\n{}",
                self.persona
            )));
        }
        // The agent's current inner state (mood / energy / familiarity).
        if !self.mood_flavor.trim().is_empty() {
            messages.push(Message::system(self.mood_flavor.clone()));
        }
        // Tell the agent it owns a persistent ICM memory it recalls and writes to.
        messages.push(Message::system(
            "You have a persistent long-term memory (ICM). Before acting, the \
             part of it that matters is recalled for you: the expertise you \
             have distilled from experience, then what you remember that \
             relates to this task. Each line starts with its importance in \
             brackets. What you conclude is saved back to it automatically. \
             Build on what you already learned; do not contradict it and avoid \
             repeating it."
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
                "Your recalled memory:\n\n{recalled}"
            )));
        } else {
            messages.push(Message::system(
                "Your memory is currently empty for this objective — what you \
                 conclude now becomes your first memories."
                    .to_string(),
            ));
        }
        messages.push(Message::user(input.to_string()));
        let mut req = CompletionRequest::new(messages);
        req.max_tokens = self.max_tokens;

        // A provider failure fails the step (and the run). Only demo mode
        // substitutes the offline canned provider, and the run is then flagged
        // so its output is never billed or alerted on.
        let mut used_canned = false;
        let completion = match provider.complete(req.clone()).await {
            Ok(c) => c,
            Err(e) if self.registry.demo_mode() => {
                tracing::warn!(step = step.as_str(), error = %e, "step failed, canned fallback (demo mode)");
                bus.publish(JobEvent::log(
                    &self.job.id,
                    format!(
                        "{} provider error, using offline demo fallback",
                        label(step)
                    ),
                ));
                used_canned = true;
                self.registry.canned().complete(req).await?
            }
            Err(e) => {
                bus.publish(JobEvent::log(
                    &self.job.id,
                    format!("{} provider error: {e}", label(step)),
                ));
                return Err(e).with_context(|| format!("{} step failed", label(step)));
            }
        };

        self.record_usage(provider.name(), &completion.model, completion.usage)
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

        // ── ICM store (opt-in per step) ────────────────────────────────────
        // Persist this step's result only when the step config asks for it
        // (`remember: true`): by default only the run summary and the user's
        // interaction are stored, at the end of the run. Skipped for read-only
        // (marketplace) runs to protect the publisher, and when the canned demo
        // fallback was used (its generic content would poison recall).
        if !used_canned && self.remember(step) {
            let scope = &self.write_scope;
            let trimmed: String = completion.content.chars().take(500).collect();
            let prov = crate::memory::Provenance::for_run(scope, &self.job.id);
            if let Err(e) = self
                .state
                .memory
                .store_with(scope, step.as_str(), &trimmed, &prov)
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
        self.last_step_canned = used_canned;
        self.any_step_canned |= used_canned;
        Ok(completion.content)
    }

    async fn record_usage(&self, provider: &str, model: &str, usage: TokenUsage) {
        let cost = self.state.config.pricing.cost_usd(model, usage);
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
