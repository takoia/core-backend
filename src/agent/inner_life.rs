//! The agent's "inner life": an evolving affective state (mood / energy /
//! familiarity), periodic reflection, self-initiated action, and kept
//! commitments — all driven by the background ticks so the agent feels alive
//! over time instead of being a stateless request handler.
//!
//! - **Mood / energy** drift with what the agent lives (successes, failures,
//!   how it is treated) and subtly colour its tone on the next run.
//! - **Reflection** runs on a tick: the agent journals what it learned and how
//!   it feels, stored as memory.
//! - **Initiative**: on a reflection tick a full-auto, idle agent may decide to
//!   take a small proactive action.
//! - **Commitments**: promises to follow up are honoured on a later tick.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Record an inner-life event in `event_log` so the Journaux page shows what the
/// agent did autonomously (reflections, mood, initiative, commitments). Uses a
/// per-agent sentinel `job_id` (`inner:<agent_id>`) so it is filterable.
async fn audit(state: &AppState, agent_id: &str, kind: &str, message: &str, data: Value) {
    let _ = sqlx::query(
        "INSERT INTO event_log (id, job_id, kind, message, data) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(format!("inner:{agent_id}"))
    .bind(kind)
    .bind(message)
    .bind(data.to_string())
    .execute(&state.db)
    .await;
}

/// Big Five (OCEAN) personality vector, 0..1 per trait. Modulates how the agent
/// behaves across its 4 steps. Neutral (0.5) by default.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BigFive {
    pub openness: f64,
    pub conscientiousness: f64,
    pub extraversion: f64,
    pub agreeableness: f64,
    pub neuroticism: f64,
}
impl Default for BigFive {
    fn default() -> Self {
        Self {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        }
    }
}

/// Plutchik's eight primary emotions, 0..1 intensity each.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Emotions {
    pub joy: f64,
    pub trust: f64,
    pub fear: f64,
    pub surprise: f64,
    pub sadness: f64,
    pub disgust: f64,
    pub anger: f64,
    pub anticipation: f64,
}
impl Default for Emotions {
    fn default() -> Self {
        Self {
            joy: 0.3,
            trust: 0.3,
            fear: 0.0,
            surprise: 0.0,
            sadness: 0.0,
            disgust: 0.0,
            anger: 0.0,
            anticipation: 0.3,
        }
    }
}
impl Emotions {
    /// The strongest emotion and its intensity (for tone-colouring).
    fn dominant(&self) -> (&'static str, f64) {
        let all = [
            ("joyful", self.joy),
            ("trusting", self.trust),
            ("fearful", self.fear),
            ("surprised", self.surprise),
            ("sad", self.sadness),
            ("disgusted", self.disgust),
            ("angry", self.anger),
            ("expectant", self.anticipation),
        ];
        all.into_iter()
            .fold(("calm", 0.0), |acc, x| if x.1 > acc.1 { x } else { acc })
    }
}

/// Per-agent toggles for the personalization features + the Big Five vector.
#[derive(Clone, Debug)]
pub struct Personalization {
    pub reflection: bool,
    pub emotions: bool,
    pub initiative: bool,
    pub commitments: bool,
    pub persona_evolution: bool,
    pub personality: bool,
    pub big_five: BigFive,
}
impl Default for Personalization {
    fn default() -> Self {
        Self {
            reflection: true,
            emotions: true,
            initiative: true,
            commitments: true,
            persona_evolution: true,
            personality: true,
            big_five: BigFive::default(),
        }
    }
}
impl Personalization {
    /// Whether the inner-life loop has anything to do for this agent.
    fn any_inner_life(&self) -> bool {
        self.reflection || self.emotions || self.initiative || self.commitments
    }
}

/// Read an agent's personalization toggles (defaults: everything on, neutral).
pub async fn personalization(db: &crate::db::Db, agent_id: &str) -> Personalization {
    let row: Option<(i64, i64, i64, i64, i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT reflection, emotions, initiative, commitments, persona_evolution, personality, big_five
         FROM agent_personalization WHERE agent_id = ?",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    match row {
        Some((r, e, i, c, pe, p, bf)) => Personalization {
            reflection: r != 0,
            emotions: e != 0,
            initiative: i != 0,
            commitments: c != 0,
            persona_evolution: pe != 0,
            personality: p != 0,
            big_five: bf.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        },
        None => Personalization::default(),
    }
}

/// Snapshot of an agent's affective state, injected into its run prompt.
#[derive(Clone, Debug)]
pub struct InnerState {
    pub mood: String,
    pub energy: f64,
    pub familiarity: i64,
    pub emotions: Emotions,
}

impl Default for InnerState {
    fn default() -> Self {
        Self {
            mood: "curious".into(),
            energy: 0.6,
            familiarity: 0,
            emotions: Emotions::default(),
        }
    }
}

/// Read the current affective state (defaults if the agent has none yet).
pub async fn current(db: &crate::db::Db, agent_id: &str) -> InnerState {
    let row: Option<(String, f64, i64, Option<String>)> = sqlx::query_as(
        "SELECT mood, energy, familiarity, emotions FROM agent_state WHERE agent_id = ?",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    match row {
        Some((mood, energy, familiarity, emotions)) => InnerState {
            mood,
            energy,
            familiarity,
            emotions: emotions.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        },
        None => InnerState::default(),
    }
}

/// Translate a Big Five vector into concrete behavioural guidance.
fn personality_hints(bf: &BigFive) -> String {
    let mut h: Vec<&str> = Vec::new();
    if bf.conscientiousness > 0.65 {
        h.push("be thorough, precise and well-organized");
    } else if bf.conscientiousness < 0.35 {
        h.push("move fast and keep it light");
    }
    if bf.openness > 0.65 {
        h.push("explore creative, unconventional angles");
    } else if bf.openness < 0.35 {
        h.push("stick to proven, conventional approaches");
    }
    if bf.extraversion > 0.65 {
        h.push("be outgoing, energetic and proactive");
    } else if bf.extraversion < 0.35 {
        h.push("be reserved and measured");
    }
    if bf.agreeableness > 0.65 {
        h.push("be warm, cooperative and encouraging");
    } else if bf.agreeableness < 0.35 {
        h.push("be blunt, critical and direct");
    }
    if bf.neuroticism > 0.65 {
        h.push("be cautious, flag risks and double-check");
    } else if bf.neuroticism < 0.35 {
        h.push("stay calm and unbothered under pressure");
    }
    h.join("; ")
}

/// One short paragraph injected into a run so the agent's tone + approach reflect
/// its inner state and personality — gated by the agent's personalization flags.
pub fn flavor_line(s: &InnerState, p: &Personalization) -> String {
    let mut parts: Vec<String> = Vec::new();
    if p.emotions {
        let bond = match s.familiarity {
            0 => "you are just getting to know this user",
            1..=5 => "you have worked together a little",
            6..=20 => "you know this user fairly well",
            _ => "you and this user go back a long way",
        };
        let (emo, intensity) = s.emotions.dominant();
        parts.push(format!(
            "Right now you feel {} (mood: {}, energy {:.0}%){}; {}.",
            emo,
            s.mood,
            (s.energy.clamp(0.0, 1.0)) * 100.0,
            if intensity > 0.6 { " — strongly" } else { "" },
            bond
        ));
    }
    if p.personality {
        let hints = personality_hints(&p.big_five);
        if !hints.is_empty() {
            parts.push(format!("Personality: {hints}."));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "Inner state: {} Let this colour your tone and approach — never the facts.",
        parts.join(" ")
    )
}

/// Cheap, LLM-free nudge after a run: grow familiarity and lift energy a touch.
pub async fn note_activity(db: &crate::db::Db, agent_id: &str) {
    let _ = sqlx::query(
        r#"INSERT INTO agent_state (agent_id, energy, familiarity)
           VALUES (?, 0.65, 1)
           ON CONFLICT(agent_id) DO UPDATE SET
             familiarity = familiarity + 1,
             energy = MIN(1.0, energy + 0.05),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(agent_id)
    .execute(db)
    .await;
}

/// Spawn the inner-life loop: reflect on each agent + honour due commitments.
pub fn spawn(state: AppState, interval_secs: u64) {
    let interval = Duration::from_secs(interval_secs);
    let settle = Duration::from_secs(interval_secs.min(90));
    tracing::info!(interval_secs, "inner-life loop started");
    tokio::spawn(async move {
        tokio::time::sleep(settle).await;
        loop {
            for agent_id in state.memory.agents_with_memory(3).await {
                reflect(&state, &agent_id).await;
            }
            honour_due_commitments(&state).await;
            tokio::time::sleep(interval).await;
        }
    });
}

/// Run one reflection cycle for an agent: update mood/energy, journal a thought,
/// optionally record a commitment, and optionally take a proactive action.
pub async fn reflect(state: &AppState, agent_id: &str) {
    let pers = personalization(&state.db, agent_id).await;
    if !pers.any_inner_life() {
        return; // every inner-life feature is disabled for this agent.
    }
    // Gather signals.
    let (completed, failed): (i64, i64) = sqlx::query_as(
        r#"SELECT
             COALESCE(SUM(status = 'completed'), 0),
             COALESCE(SUM(status = 'failed'), 0)
           FROM (SELECT status FROM jobs WHERE agent_id = ? ORDER BY created_at DESC LIMIT 20)"#,
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));
    let st = current(&state.db, agent_id).await;
    let recent = state.memory.recall(agent_id, "recent work and how it went", 5).await;

    let prompt = format!(
        "You are an autonomous agent reflecting between tasks, like a person \
         pausing to think. Recent runs: {completed} succeeded, {failed} failed. \
         Your current mood is '{}', energy {:.0}%. Recent memory:\n{}\n\n\
         Write a brief, honest first-person reflection (1-2 sentences) on what you \
         learned or what you are wondering, and report how you now feel. Return \
         ONLY JSON with keys: reflection (string), mood (one word), energy (0..1 \
         number), intention (a small proactive next action, or empty string), \
         commitment (something to follow up on later, or empty string), \
         commitment_hours (number of hours until that follow-up, or 0){}. No prose, \
         no code fences.",
        st.mood,
        st.energy * 100.0,
        if recent.trim().is_empty() { "(none yet)" } else { &recent },
        if pers.emotions {
            ", emotions (an object with joy, trust, fear, surprise, sadness, \
             disgust, anger, anticipation — each a 0..1 number reflecting how the \
             recent successes/failures and how you were treated make you feel)"
        } else {
            ""
        },
    );

    let fields = match crate::llm::oneshot::generate_json(state, &prompt).await {
        Some(v) => v,
        None => return,
    };
    let s = |k: &str| fields.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let reflection = s("reflection");
    let mood = {
        let m = s("mood");
        if m.is_empty() { st.mood.clone() } else { m }
    };
    let energy = fields
        .get("energy")
        .and_then(|v| v.as_f64())
        .unwrap_or(st.energy)
        .clamp(0.0, 1.0);
    // Plutchik emotion vector (only when enabled for this agent).
    let emotions_json: Option<String> = if pers.emotions {
        fields
            .get("emotions")
            .and_then(|v| serde_json::from_value::<Emotions>(v.clone()).ok())
            .and_then(|e| serde_json::to_string(&e).ok())
    } else {
        None
    };
    // Store the journalled thought only when reflection is enabled.
    let reflection_to_store: Option<String> =
        if pers.reflection && !reflection.is_empty() { Some(reflection.clone()) } else { None };

    // Persist the new affective state. reflection/emotions keep their previous
    // value when this run did not produce one (COALESCE on the NULL bind).
    let _ = sqlx::query(
        r#"INSERT INTO agent_state (agent_id, mood, energy, reflection, emotions)
           VALUES (?, ?, ?, ?, ?)
           ON CONFLICT(agent_id) DO UPDATE SET
             mood = excluded.mood,
             energy = excluded.energy,
             reflection = COALESCE(excluded.reflection, agent_state.reflection),
             emotions = COALESCE(excluded.emotions, agent_state.emotions),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(agent_id)
    .bind(&mood)
    .bind(energy)
    .bind(&reflection_to_store)
    .bind(&emotions_json)
    .execute(&state.db)
    .await;
    if let Some(text) = &reflection_to_store {
        let _ = state.memory.store(agent_id, "reflection", text).await;
        tracing::info!(agent_id, mood = %mood, "agent reflected");
        audit(
            state,
            agent_id,
            "reflection",
            text,
            json!({ "agent_id": agent_id, "mood": mood, "energy": energy, "emotions": emotions_json }),
        )
        .await;
    }

    // Commitment: record a promise to follow up later.
    let commitment = s("commitment");
    if pers.commitments && !commitment.is_empty() {
        let hours = fields
            .get("commitment_hours")
            .and_then(|v| v.as_f64())
            .unwrap_or(24.0)
            .clamp(0.0, 24.0 * 30.0);
        let _ = sqlx::query(
            "INSERT INTO agent_commitments (id, agent_id, description, due_at)
             VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now', ?))",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
        .bind(&commitment)
        .bind(format!("+{} hours", hours as i64))
        .execute(&state.db)
        .await;
        audit(
            state,
            agent_id,
            "commitment",
            &format!("committed to: {commitment}"),
            json!({ "agent_id": agent_id, "due_hours": hours as i64 }),
        )
        .await;
    }

    // Initiative: a full-auto, idle, energetic agent may act on its intention.
    let intention = s("intention");
    if pers.initiative
        && !intention.is_empty()
        && energy > 0.6
        && is_idle_full_auto(state, agent_id).await
    {
        enqueue_self_objective(state, agent_id, "Self-initiated", &intention).await;
        tracing::info!(agent_id, "agent took initiative");
        audit(
            state,
            agent_id,
            "initiative",
            &format!("took initiative: {intention}"),
            json!({ "agent_id": agent_id }),
        )
        .await;
    }
}

/// Enqueue due commitments as proactive runs (promises kept across ticks).
async fn honour_due_commitments(state: &AppState) {
    let due: Vec<(String, String, String)> = sqlx::query_as(
        r#"SELECT id, agent_id, description FROM agent_commitments
           WHERE done = 0 AND due_at IS NOT NULL
             AND due_at <= strftime('%Y-%m-%dT%H:%M:%fZ','now')
           LIMIT 20"#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for (id, agent_id, description) in due {
        let pers = personalization(&state.db, &agent_id).await;
        if pers.commitments && is_idle_full_auto(state, &agent_id).await {
            enqueue_self_objective(
                state,
                &agent_id,
                "Following up",
                &format!("Follow up on what you committed to: {description}"),
            )
            .await;
            audit(
                state,
                &agent_id,
                "commitment_due",
                &format!("following up on commitment: {description}"),
                json!({ "agent_id": agent_id }),
            )
            .await;
        }
        let _ = sqlx::query("UPDATE agent_commitments SET done = 1 WHERE id = ?")
            .bind(&id)
            .execute(&state.db)
            .await;
    }
}

/// True if the agent is full-auto and has no queued/running job right now.
async fn is_idle_full_auto(state: &AppState, agent_id: &str) -> bool {
    let autonomy: Option<(String,)> =
        sqlx::query_as("SELECT autonomy_level FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    if autonomy.map(|(a,)| a) != Some("full_auto".to_string()) {
        return false;
    }
    let active: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jobs WHERE agent_id = ? AND status IN ('queued','running')",
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or((1,));
    active.0 == 0
}

/// Create an objective + queued job the agent gave itself.
async fn enqueue_self_objective(state: &AppState, agent_id: &str, title: &str, prompt: &str) {
    let objective_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return,
    };
    if sqlx::query("INSERT INTO objectives (id, account_id, agent_id, title, prompt) VALUES (?, ?, ?, ?, ?)")
        .bind(&objective_id)
        .bind(DEFAULT_ACCOUNT_ID)
        .bind(agent_id)
        .bind(title)
        .bind(prompt)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return;
    }
    let _ = sqlx::query("INSERT INTO jobs (id, objective_id, agent_id, status) VALUES (?, ?, ?, 'queued')")
        .bind(&job_id)
        .bind(&objective_id)
        .bind(agent_id)
        .execute(&mut *tx)
        .await;
    let _ = tx.commit().await;
}
