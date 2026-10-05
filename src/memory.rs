//! Permanent agent memory — the "personalize your AI over time" capability.
//!
//! Backed by ICM (`icm` CLI) for recall, with the `memories` table as the
//! always-available source of truth for the UI and as a fallback if ICM is
//! unavailable. Each agent owns an ICM topic, so an expert agent (e.g. trading)
//! accumulates and refines expertise across runs.
//!
//! Memory is scoped ([`MemoryScope`]): the owner's memory lives in
//! `takoia/agent/{id}`; every marketplace consumer gets a fork of their own in
//! `takoia/fork/{id}/{account}` and writes only there, so the agent becomes
//! theirs without their inputs ever reaching the publisher's expertise.
//!
//! It is also layered. Those two scopes hold EPISODES: what was learnt, as it
//! was learnt, never deleted by maintenance. `takoia/know/{id}` holds the
//! KNOWLEDGE distilled from the owner's episodes — the transferable expertise,
//! stripped of the trainer's private history (see [`crate::distill`]).
//! `memory_derivations` records which episodes each knowledge row came from, so
//! erasing an episode takes what was derived from it along.
//!
//! A run recalls the knowledge, then its own episodes: the owner's for the
//! owner's run, the consumer's fork for a marketplace run — which never reads
//! the owner's episodes (see [`Memory::recall_composed`]).

use crate::db::Db;
use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::process::Command;
use uuid::Uuid;

/// An ICM memory with its native importance metadata.
#[derive(Debug, Clone, Serialize)]
pub struct IcmEntry {
    /// ICM's own id, so a mirrored row can be erased there.
    pub id: Option<String>,
    pub summary: String,
    pub weight: f64,
    pub access_count: i64,
    pub importance: String,
}

/// A stored memory entry, surfaced in the UI and the consumer API.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MemoryEntry {
    pub id: String,
    pub agent_id: String,
    pub key: String,
    pub content: String,
    pub created_at: String,
    /// Where it came from: run, interaction, step, correction, manual, video,
    /// reflection, import.
    pub source: String,
    /// Whose data it is (a consumer account, a user id, an external reference).
    pub subject: Option<String>,
    /// contract | consent | legitimate_interest.
    pub legal_basis: Option<String>,
    /// ISO-8601 instant after which the row is erased by the maintenance loop.
    pub retain_until: Option<String>,
    pub job_id: Option<String>,
    /// `episode` (what was learnt, verbatim) or `knowledge` (distilled from it).
    pub layer: String,
}

/// Columns of [`MemoryEntry`], in one place so every listing stays in step.
const ENTRY_COLUMNS: &str = "id, agent_id, key, content, created_at, \
     source, subject, legal_basis, retain_until, job_id, layer";

/// Outcome of a store: the mirror row holding the content, and whether this
/// call created it (`false` = the same content was already there).
///
/// `retain_until` and `legal_basis` are what is in force on that row, which
/// for a duplicate is not necessarily what the call asked for: an existing row
/// keeps its legal basis, and its deadline can only be brought forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub id: String,
    pub created: bool,
    pub retain_until: Option<String>,
    pub legal_basis: Option<String>,
}

/// The mirror row that already holds a content (see [`Memory::find_duplicate`]).
#[derive(Debug, Clone, sqlx::FromRow)]
struct Duplicate {
    id: String,
    icm_id: Option<String>,
    retain_until: Option<String>,
    legal_basis: Option<String>,
    /// Non-zero when the row is past its retention and not swept yet.
    expired: i64,
}

/// A mirror row as the ICM side needs it: enough to store its content again.
#[derive(Debug, Clone, sqlx::FromRow)]
struct MirrorRow {
    id: String,
    agent_id: String,
    consumer_account: Option<String>,
    layer: String,
    key: String,
    content: String,
    icm_id: Option<String>,
}

impl MirrorRow {
    fn scope(&self) -> MemoryScope {
        match (&self.consumer_account, self.layer.as_str()) {
            (Some(account), _) => MemoryScope::consumer(&self.agent_id, account),
            (None, LAYER_KNOWLEDGE) => MemoryScope::knowledge(&self.agent_id),
            (None, _) => MemoryScope::owner(&self.agent_id),
        }
    }
}

/// Columns of [`MirrorRow`].
const MIRROR_ROW_COLUMNS: &str = "id, agent_id, consumer_account, layer, key, content, icm_id";

/// SQL condition: the row's retention period has ended.
const EXPIRED: &str = "retain_until IS NOT NULL \
     AND retain_until <= strftime('%Y-%m-%dT%H:%M:%fZ','now')";

/// What happens to an ICM entry when some of the mirror rows pointing at it
/// are erased (see [`icm_fate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IcmFate {
    /// No mirror row is left on it: forget it.
    Forget,
    /// The rows staying hold the very content of the rows leaving: it is
    /// theirs as much, keep it.
    Keep,
    /// It holds text of the rows leaving that the rows staying do not have
    /// (ICM folded a near-duplicate into it): forget it, then store the
    /// staying rows' own content again.
    Rebuild,
}

/// Where a memory came from, whose data it is, and how long it may be kept.
/// Stored next to the content so a single memory can be traced and erased.
#[derive(Debug, Clone, Default)]
pub struct Provenance {
    pub source: Option<String>,
    pub subject: Option<String>,
    pub legal_basis: Option<String>,
    pub retain_until: Option<String>,
    pub job_id: Option<String>,
}

impl Provenance {
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }
    pub fn basis(mut self, basis: &str) -> Self {
        self.legal_basis = Some(basis.to_string());
        self
    }
    pub fn job(mut self, job_id: &str) -> Self {
        self.job_id = Some(job_id.to_string());
        self
    }
    pub fn source(mut self, source: &str) -> Self {
        self.source = Some(source.to_string());
        self
    }
    /// Set the retention deadline from an RFC 3339 timestamp; stored normalised
    /// to UTC in the exact `strftime('%Y-%m-%dT%H:%M:%fZ')` shape the sweep
    /// compares against, so a lexical comparison is a chronological one.
    pub fn retain_until(mut self, until: &str) -> Result<Self> {
        self.retain_until = Some(normalize_deadline(until)?);
        Ok(self)
    }

    /// Provenance of what a run writes: the consumer's data under the
    /// marketplace contract for a fork, the publisher's own for owner memory.
    pub fn for_run(scope: &MemoryScope, job_id: &str) -> Self {
        let p = Provenance::default().job(job_id);
        match scope {
            MemoryScope::Consumer { account_id, .. } => {
                p.subject(account_id.clone()).basis("contract")
            }
            MemoryScope::Owner { .. } | MemoryScope::Knowledge { .. } => p,
        }
    }
}

/// Parse an RFC 3339 instant and render it like SQLite's
/// `strftime('%Y-%m-%dT%H:%M:%fZ','now')` (UTC, millisecond fraction).
pub fn normalize_deadline(input: &str) -> Result<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(input.trim())
        .map_err(|e| anyhow::anyhow!("retain_until must be an RFC 3339 timestamp: {e}"))?;
    Ok(dt
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%dT%H:%M:%S.%3fZ")
        .to_string())
}

/// Result of an erasure: rows removed from the mirror, and how many of them
/// could NOT be removed from ICM (no id known, or `icm forget` failed) — the
/// caller must surface that, an erasure that silently left ICM copies is not
/// an erasure.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct Erased {
    /// The rows that were asked for.
    pub rows: u64,
    /// Knowledge rows erased along because they were distilled from them.
    pub derived: u64,
    /// ICM copies left behind, over both kinds of rows.
    pub icm_failed: u64,
}

/// What [`Memory::forget_agent`] wiped.
#[derive(Debug, Clone)]
pub struct AgentWipe {
    /// ICM topics that could not be forgotten.
    pub icm_failed: u64,
    topics: Vec<String>,
}

/// What else must change when rows are erased (see [`Memory::erase_rows`]).
#[derive(Debug, Default)]
struct Cascade {
    /// Knowledge rows distilled from an erased row: `(id, icm_id)`.
    knowledge: Vec<(String, Option<String>)>,
    /// Surviving episodes that fed those knowledge rows: distilled again.
    requeue: Vec<String>,
}

/// Default `source` for a memory key when the caller gives none.
fn source_for_key(key: &str) -> &'static str {
    match key {
        "run-summary" => "run",
        "interaction" => "interaction",
        "correction" => "correction",
        "reflection" => "reflection",
        "video-analysis" => "video",
        "demonstration" | "preference" | "instruction" => "manual",
        "analyse" | "decision" | "action" | "restitution" => "step",
        _ => "run",
    }
}

/// ICM importance of a memory, `None` for ICM's default (medium). It only
/// drives ranking (nothing is pruned any more). User-authored signal outranks
/// the agent's own output: corrections and preferences first, run summaries
/// and self-reflections last; other (generic step) memories keep the default.
/// Distilled knowledge is what the agent is for, so it always ranks high.
fn importance_of(layer: &str, key: &str) -> Option<&'static str> {
    match (layer, key) {
        (LAYER_KNOWLEDGE, _) => Some("high"),
        (_, "correction" | "preference" | "demonstration" | "instruction") => Some("high"),
        (_, "run-summary" | "reflection") => Some("low"),
        _ => None,
    }
}

/// The id ICM printed for a stored memory: `Stored: <id> (+N links)` for a new
/// row or an exact duplicate, `Updated existing memory (similarity 0.98): <id>`
/// when embeddings folded it into a near-duplicate.
fn parse_icm_stored_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("Stored:").or_else(|| {
                l.strip_prefix("Updated existing memory")
                    .and_then(|rest| rest.rsplit_once(':'))
                    .map(|(_, id)| id)
            })
        })
        .and_then(|rest| rest.split_whitespace().next())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// De-duplication key of a memory: lowercase hex SHA-256 of the content with
/// whitespace runs collapsed, trimmed and lowercased, so a re-run that words
/// the same thing with different spacing or casing is recognised.
pub fn content_hash(content: &str) -> String {
    let normalised = content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    format!("{:x}", Sha256::digest(normalised.as_bytes()))
}

/// Whose memory a read or write addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryScope {
    /// The publisher's own episodes with the agent: `takoia/agent/{id}`.
    Owner { agent_id: String },
    /// One consumer account's fork: `takoia/fork/{id}/{account}`.
    Consumer {
        agent_id: String,
        account_id: String,
    },
    /// What was distilled from the owner's episodes: `takoia/know/{id}`.
    Knowledge { agent_id: String },
}

const TOPIC_PREFIX: &str = "takoia/agent/";
/// Forks live under a DIFFERENT root on purpose: `icm recall --topic` matches
/// topics by substring, either way round (verified against icm 0.10.65), so a
/// fork named `takoia/agent/{id}/...` would be recalled by the owner's runs and
/// by every other consumer's. `takoia/fork/{agent}/{account}` and
/// `takoia/agent/{agent}` are not substrings of one another.
const FORK_PREFIX: &str = "takoia/fork/";
/// Same reasoning for the knowledge layer: not a substring of the episode
/// topic, so an episode recall never drags distilled rows in (or the reverse).
const KNOWLEDGE_PREFIX: &str = "takoia/know/";

/// `memories.layer` of the raw, verbatim rows.
pub const LAYER_EPISODE: &str = "episode";
/// `memories.layer` of the rows distilled from episodes.
pub const LAYER_KNOWLEDGE: &str = "knowledge";

impl MemoryScope {
    pub fn owner(agent_id: &str) -> Self {
        MemoryScope::Owner {
            agent_id: agent_id.to_string(),
        }
    }

    pub fn consumer(agent_id: &str, account_id: &str) -> Self {
        MemoryScope::Consumer {
            agent_id: agent_id.to_string(),
            account_id: account_id.to_string(),
        }
    }

    pub fn knowledge(agent_id: &str) -> Self {
        MemoryScope::Knowledge {
            agent_id: agent_id.to_string(),
        }
    }

    pub fn agent_id(&self) -> &str {
        match self {
            MemoryScope::Owner { agent_id }
            | MemoryScope::Consumer { agent_id, .. }
            | MemoryScope::Knowledge { agent_id } => agent_id,
        }
    }

    /// `None` for the owner's memory (episodes and knowledge alike), the
    /// account id for a consumer fork. Also the value of
    /// `memories.consumer_account`.
    pub fn consumer_account(&self) -> Option<&str> {
        match self {
            MemoryScope::Owner { .. } | MemoryScope::Knowledge { .. } => None,
            MemoryScope::Consumer { account_id, .. } => Some(account_id),
        }
    }

    /// The value of `memories.layer`. Together with
    /// [`consumer_account`](Self::consumer_account) it selects exactly this
    /// scope's rows in the mirror.
    pub fn layer(&self) -> &'static str {
        match self {
            MemoryScope::Owner { .. } | MemoryScope::Consumer { .. } => LAYER_EPISODE,
            MemoryScope::Knowledge { .. } => LAYER_KNOWLEDGE,
        }
    }

    /// The ICM topic. Owner topics are exactly what they were before scoping,
    /// so existing ICM data needs no migration.
    pub fn topic(&self) -> String {
        match self {
            MemoryScope::Owner { agent_id } => format!("{TOPIC_PREFIX}{agent_id}"),
            MemoryScope::Consumer {
                agent_id,
                account_id,
            } => format!("{FORK_PREFIX}{agent_id}/{account_id}"),
            MemoryScope::Knowledge { agent_id } => format!("{KNOWLEDGE_PREFIX}{agent_id}"),
        }
    }

    /// Inverse of [`topic`](Self::topic). Rejects anything that is not exactly
    /// one of the three shapes (a bare `strip_prefix` would read a consumer
    /// topic as an owner topic with a bogus agent id).
    pub fn parse_topic(topic: &str) -> Option<MemoryScope> {
        if let Some(agent) = topic.strip_prefix(TOPIC_PREFIX) {
            return (!agent.is_empty() && !agent.contains('/')).then(|| MemoryScope::owner(agent));
        }
        if let Some(agent) = topic.strip_prefix(KNOWLEDGE_PREFIX) {
            return (!agent.is_empty() && !agent.contains('/'))
                .then(|| MemoryScope::knowledge(agent));
        }
        let rest = topic.strip_prefix(FORK_PREFIX)?;
        let (agent, account) = rest.split_once('/')?;
        (!agent.is_empty() && !account.is_empty() && !account.contains('/'))
            .then(|| MemoryScope::consumer(agent, account))
    }
}

/// One async lock per agent, shared by every clone of [`Memory`]. The std mutex
/// only guards the map and is never held across an await.
#[derive(Clone, Default)]
struct AgentLocks(Arc<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>);

impl AgentLocks {
    fn get(&self, agent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        // A poisoned map is still a valid map: no invariant spans the guard.
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(agent_id.to_string()).or_default().clone()
    }

    /// Drop a deleted agent's entry, unless someone still holds or awaits it.
    fn release(&self, agent_id: &str) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(agent_id).is_some_and(|l| Arc::strong_count(l) == 1) {
            map.remove(agent_id);
        }
    }
}

/// Proof that the holder owns one agent's memory lock (see
/// [`Memory::lock_agent`]). Erasure takes it by reference, so a caller that
/// already holds the lock can erase without deadlocking on it.
pub struct AgentGuard {
    agent_id: String,
    _held: tokio::sync::OwnedMutexGuard<()>,
}

impl AgentGuard {
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

/// An agent's lock for a step that needs it: the caller's when it already
/// holds it, one taken on the spot otherwise.
enum Held<'a> {
    Caller(&'a AgentGuard),
    Taken(AgentGuard),
}

impl Held<'_> {
    fn guard(&self) -> &AgentGuard {
        match self {
            Held::Caller(guard) => guard,
            Held::Taken(guard) => guard,
        }
    }
}

/// Memory store bridging ICM and the local `memories` table.
#[derive(Clone)]
pub struct Memory {
    db: Db,
    icm_db_path: String,
    /// `MEMORY_EMBEDDINGS`: let ICM embed on store and recall (off by default).
    embeddings: bool,
    locks: AgentLocks,
}

/// Prompt budget of the knowledge block, in chars, spent on whole entries.
const KNOWLEDGE_RECALL_CHARS: usize = 2000;
/// Prompt budget of the owner's episode block (owner runs only).
const EPISODE_RECALL_CHARS: usize = 2000;
/// Prompt budget of a consumer's own fork block (consumer runs only).
const FORK_RECALL_CHARS: usize = 2000;
/// Prompt budget of a single-scope [`Memory::recall`].
const SCOPE_RECALL_CHARS: usize = 4000;
/// Knowledge rows asked for per run; the budget decides how many are kept.
const KNOWLEDGE_RECALL_ENTRIES: usize = 24;
/// `icm recall --topic` matches sibling topics too and applies `--limit`
/// before the exact-topic filter done here, so more is asked for than wanted.
const RECALL_OVERFETCH: usize = 4;

const KNOWLEDGE_HEADING: &str = "What you know (expertise distilled from your experience):";
const EPISODE_HEADING: &str = "What you remember from past work:";
const FORK_HEADING: &str = "What you have learnt about this user (their own history with you):";
/// Rows per `IN (...)` list, well under SQLite's bound-parameter limit.
const SQL_IN_CHUNK: usize = 500;
/// Mirror rows without an ICM copy that one maintenance pass tries again.
const ICM_BACKFILL_PER_PASS: i64 = 25;

impl Memory {
    pub fn new(db: Db, icm_db_path: String) -> Self {
        Self {
            db,
            icm_db_path,
            embeddings: false,
            locks: AgentLocks::default(),
        }
    }

    /// Opt into ICM embeddings (`MEMORY_EMBEDDINGS=true`). Experimental: every
    /// `icm` call then loads the embedding model, seconds instead of
    /// milliseconds per call.
    pub fn with_embeddings(mut self, enabled: bool) -> Self {
        self.embeddings = enabled;
        self
    }

    /// The one place an `icm` command is built: the binary, the dedicated
    /// database, and keyword-only mode unless embeddings were opted into.
    /// Callers append the subcommand and its arguments.
    fn icm(&self) -> Command {
        let mut cmd = Command::new("icm");
        cmd.arg("--db").arg(&self.icm_db_path);
        if !self.embeddings {
            cmd.arg("--no-embeddings");
        }
        cmd
    }

    /// The mirror's database, for the distillation pass that works on it.
    pub(crate) fn db(&self) -> &Db {
        &self.db
    }

    /// Take `agent_id`'s memory lock. Held by everything that rewrites an
    /// agent's memory as a whole — distillation (while it reads its snapshot
    /// and while it writes, not while the model answers) and every erasure
    /// path — so an erasure never interleaves with either. `store_with` does
    /// not take it: appending an episode is always safe.
    pub async fn lock_agent(&self, agent_id: &str) -> AgentGuard {
        let lock = self.locks.get(agent_id);
        AgentGuard {
            agent_id: agent_id.to_string(),
            _held: lock.lock_owned().await,
        }
    }

    /// `agent_id`'s lock for one step: `held` if the caller has it already.
    async fn hold<'a>(&self, held: Option<&'a AgentGuard>, agent_id: &str) -> Held<'a> {
        match held {
            Some(guard) => Held::Caller(guard),
            None => Held::Taken(self.lock_agent(agent_id).await),
        }
    }

    /// What a run recalls, as headed blocks of `- [importance] summary` lines.
    ///
    /// Every run gets the KNOWLEDGE block first: the top-weight entries of
    /// what was distilled from the publisher's episodes. Then comes the
    /// personal block — the owner's episodes for the owner's own run, the
    /// consumer's fork for a marketplace run — so the freshest, most specific
    /// context sits last, right before the task. A consumer run never reads
    /// the owner's episode scope: what a buyer gets is the distilled
    /// expertise, not the trainer's history.
    ///
    /// Each block has its own budget, spent on whole entries (see
    /// [`within_budget`]); an entry already shown by the knowledge block is
    /// not repeated.
    pub async fn recall_composed(
        &self,
        agent_id: &str,
        consumer_account: Option<&str>,
        query: &str,
        limit: usize,
    ) -> String {
        let (personal, heading, budget) = match consumer_account {
            Some(account) => (
                MemoryScope::consumer(agent_id, account),
                FORK_HEADING,
                FORK_RECALL_CHARS,
            ),
            None => (
                MemoryScope::owner(agent_id),
                EPISODE_HEADING,
                EPISODE_RECALL_CHARS,
            ),
        };
        // Independent lookups (a few `icm` spawns each): run them together.
        let knowledge_scope = MemoryScope::knowledge(agent_id);
        let (knowledge, personal) = tokio::join!(
            self.entries(&knowledge_scope, None, KNOWLEDGE_RECALL_ENTRIES),
            self.entries(&personal, Some(query), limit)
        );
        // Budget first for the knowledge: only what its block really shows may
        // hide a personal entry.
        let mut shown = HashSet::new();
        let knowledge = distinct(within_budget(knowledge, KNOWLEDGE_RECALL_CHARS), &mut shown);
        let personal = within_budget(distinct(personal, &mut shown), budget);
        compose_blocks(&[
            (KNOWLEDGE_HEADING, render_entries(&knowledge)),
            (heading, render_entries(&personal)),
        ])
    }

    /// Recall what ONE scope holds that is relevant to `query`, as
    /// `- [importance] summary` lines within a budget spent on whole entries.
    /// No other scope or layer is read: a caller that wants the distilled
    /// knowledge too asks for [`recall_composed`](Self::recall_composed).
    pub async fn recall(&self, scope: &MemoryScope, query: &str, limit: usize) -> String {
        let entries = self.entries(scope, Some(query), limit).await;
        render_entries(&within_budget(
            distinct(entries, &mut HashSet::new()),
            SCOPE_RECALL_CHARS,
        ))
    }

    /// Up to `limit` entries of one scope, from the best source that has any:
    /// 1) ICM hits for `query` (`None`: the scope is recalled whole, not
    ///    searched — the knowledge layer);
    /// 2) keyword recall frequently misses (a query shares no term with the
    ///    memories), so the scope's top-weight entries, which need no match —
    ///    accumulated expertise is ALWAYS injected;
    /// 3) the mirror, when ICM is missing or holds nothing for the topic.
    async fn entries(
        &self,
        scope: &MemoryScope,
        query: Option<&str>,
        limit: usize,
    ) -> Vec<Recalled> {
        if limit == 0 {
            return Vec::new();
        }
        if let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) {
            let hits = self.icm_hits(scope, query, limit).await;
            if !hits.is_empty() {
                return hits;
            }
        }
        let top = self.icm_top(scope, limit).await;
        if !top.is_empty() {
            return top;
        }
        match self.mirror_entries(scope, limit).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(topic = %scope.topic(), error = %e, "memory mirror recall failed");
                Vec::new()
            }
        }
    }

    /// True when a TOON recall payload actually carries rows. ICM emits a header
    /// like `memories[0]{id,topic,...}:` (note the `[0]`) with no rows when
    /// nothing matched — that header is non-empty but must be treated as empty.
    fn toon_has_entries(toon: &str) -> bool {
        let t = toon.trim();
        if t.is_empty() {
            return false;
        }
        if let Some(rest) = t.strip_prefix("memories[") {
            if let Some(end) = rest.find(']') {
                return rest[..end].trim() != "0";
            }
        }
        // Unknown shape: only trust it if there is more than the header line.
        t.lines().count() > 1
    }

    /// Stdout of a read-only `icm` call that must print a JSON list, or `None`
    /// when ICM could not run or failed (the caller falls back).
    async fn icm_json(&self, mut cmd: Command, what: &str, topic: &str) -> Option<String> {
        match cmd.output().await {
            Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(o) => {
                tracing::warn!(topic, stderr = %String::from_utf8_lossy(&o.stderr), "icm {what} failed, falling back");
                None
            }
            Err(e) => {
                tracing::debug!(topic, error = %e, "icm {what} could not run, falling back");
                None
            }
        }
    }

    /// Highest-weight entries of exactly this scope's topic, independent of
    /// any query. `icm list --topic` is an exact match and needs no keyword
    /// hit, so it reliably surfaces what the scope holds.
    async fn icm_top(&self, scope: &MemoryScope, limit: usize) -> Vec<Recalled> {
        let topic = scope.topic();
        let mut cmd = self.icm();
        cmd.arg("list")
            .arg("--topic")
            .arg(&topic)
            .arg("--sort")
            .arg("weight")
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--format")
            .arg("json");
        let Some(json) = self.icm_json(cmd, "list", &topic).await else {
            return Vec::new();
        };
        let mut entries = parse_recalled(&json, &topic);
        entries.truncate(limit);
        entries
    }

    /// Query-scoped recall, filtered to EXACTLY this scope's topic. `icm recall
    /// --topic` matches topics by substring, so without the filter a slug agent
    /// id would pull in its longer siblings (`invoice-bot` ⊂ `invoice-bot-v2`);
    /// and since ICM cuts to `--limit` first, more is asked for than wanted.
    async fn icm_hits(&self, scope: &MemoryScope, query: &str, limit: usize) -> Vec<Recalled> {
        let topic = scope.topic();
        let mut cmd = self.icm();
        cmd.arg("recall")
            .arg("--topic")
            .arg(&topic)
            .arg("--limit")
            .arg(limit.saturating_mul(RECALL_OVERFETCH).to_string())
            .arg("--format")
            .arg("json")
            // The query is free text: after `--` it is never read as a flag.
            .arg("--")
            .arg(query);
        let Some(json) = self.icm_json(cmd, "recall", &topic).await else {
            return Vec::new();
        };
        let mut entries = parse_recalled(&json, &topic);
        entries.truncate(limit);
        entries
    }

    /// The scope's most recent mirror rows, in the shape ICM would give them.
    async fn mirror_entries(&self, scope: &MemoryScope, limit: usize) -> Result<Vec<Recalled>> {
        // `IS ?` so a NULL bind matches the owner rows (`= NULL` never matches).
        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT key, content, icm_id FROM memories
             WHERE agent_id = ? AND consumer_account IS ? AND layer = ?
             ORDER BY created_at DESC, rowid DESC LIMIT ?",
        )
        .bind(scope.agent_id())
        .bind(scope.consumer_account())
        .bind(scope.layer())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, content, _)| !content.trim().is_empty())
            .map(|(key, content, icm_id)| Recalled {
                icm_id,
                importance: importance_of(scope.layer(), &key)
                    .unwrap_or("medium")
                    .to_string(),
                summary: content.trim().to_string(),
            })
            .collect())
    }

    /// Salient keywords derived from memory content so keyword recall can match
    /// content queries (not just the generic step-name key). Lowercases, splits
    /// on non-alphanumeric chars, drops short tokens and common stopwords,
    /// dedupes, and caps the list so the keyword set stays focused.
    fn content_keywords(content: &str) -> Vec<String> {
        // A small English + French stopword set: high-frequency, low-signal
        // tokens (>= 4 chars) that would otherwise dilute keyword recall.
        const STOPWORDS: &[&str] = &[
            "this", "that", "with", "from", "have", "they", "them", "then", "their", "there",
            "would", "could", "should", "about", "which", "when", "what", "were", "will", "your",
            "pour", "dans", "avec", "les", "des", "une", "que", "qui", "est", "sont", "cette",
            "vous", "nous", "mais", "comme", "plus",
        ];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for token in content.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if token.len() < 4 || STOPWORDS.contains(&token) {
                continue;
            }
            if seen.insert(token.to_string()) {
                out.push(token.to_string());
                if out.len() >= 8 {
                    break;
                }
            }
        }
        out
    }

    /// Persist a new memory with default provenance (source derived from `key`).
    pub async fn store(&self, scope: &MemoryScope, key: &str, content: &str) -> Result<()> {
        self.store_with(scope, key, content, &Provenance::default())
            .await
            .map(|_| ())
    }

    /// The mirror row already holding this content for this scope and subject.
    /// Written with the same `COALESCE`s as the unique index it relies on.
    async fn find_duplicate(
        &self,
        scope: &MemoryScope,
        subject: Option<&str>,
        hash: &str,
    ) -> Result<Option<Duplicate>> {
        Ok(sqlx::query_as(&format!(
            "SELECT id, icm_id, retain_until, legal_basis, ({EXPIRED}) AS expired
             FROM memories
             WHERE agent_id = ? AND COALESCE(consumer_account, '') = COALESCE(?, '')
               AND layer = ? AND COALESCE(subject, '') = COALESCE(?, '')
               AND content_hash = ?"
        ))
        .bind(scope.agent_id())
        .bind(scope.consumer_account())
        .bind(scope.layer())
        .bind(subject)
        .bind(hash)
        .fetch_optional(&self.db)
        .await?)
    }

    /// `icm store` of one content in `scope`'s topic: `Ok(Some(id))` with
    /// ICM's id for it, `Ok(None)` when ICM ran but refused or printed no id,
    /// `Err` when it could not be run at all (not installed). Failures are
    /// logged and nothing more: ICM is best-effort, it must never break a run.
    async fn icm_store(
        &self,
        scope: &MemoryScope,
        key: &str,
        content: &str,
    ) -> std::io::Result<Option<String>> {
        // Keyword set: the key first (generic step name), then salient terms
        // derived from the content so keyword recall can match content queries.
        let mut keywords = vec![key.to_string()];
        keywords.extend(Self::content_keywords(content));
        let mut cmd = self.icm();
        cmd.arg("store")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--content")
            .arg(content)
            .arg("--keywords")
            .arg(keywords.join(","));
        if let Some(level) = importance_of(scope.layer(), key) {
            cmd.arg("--importance").arg(level);
        }
        let agent_id = scope.agent_id();
        match cmd.output().await {
            Ok(o) if o.status.success() => {
                Ok(parse_icm_stored_id(&String::from_utf8_lossy(&o.stdout)))
            }
            Ok(o) => {
                tracing::warn!(agent_id, stderr = %String::from_utf8_lossy(&o.stderr), "icm store failed (db still persisted)");
                Ok(None)
            }
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "icm store failed (db still persisted)");
                Err(e)
            }
        }
    }

    /// Record the ICM id of a mirror row, once ICM holds its content.
    ///
    /// A row that is gone by then — erased, or cascaded away with its agent,
    /// while ICM was being written — must not leave that copy behind: nothing
    /// in the mirror would ever lead an erasure to it. It is released like any
    /// erased row's copy (see [`release_icm`](Self::release_icm)).
    pub(crate) async fn attach_icm_id(
        &self,
        held: Option<&AgentGuard>,
        agent_id: &str,
        row_id: &str,
        content: &str,
        icm_id: &str,
    ) -> Result<()> {
        let attached = sqlx::query("UPDATE memories SET icm_id = ? WHERE id = ?")
            .bind(icm_id)
            .bind(row_id)
            .execute(&self.db)
            .await?
            .rows_affected();
        if attached > 0 {
            return Ok(());
        }
        let left_behind = {
            let held = self.hold(held, agent_id).await;
            self.release_icm(
                held.guard(),
                &HashMap::from([(icm_id.to_string(), 1)]),
                &HashSet::new(),
                &[(icm_id.to_string(), content_hash(content))],
            )
            .await?
        };
        if held.is_none() {
            // The agent may be gone for good: do not keep a lock for it.
            self.locks.release(agent_id);
        }
        if left_behind > 0 {
            tracing::warn!(
                agent_id,
                icm_id,
                "a memory erased while it was being stored keeps its ICM copy"
            );
        }
        Ok(())
    }

    /// Give a mirror row that never reached ICM its copy there. `None` when
    /// `icm` cannot be run at all, else whether the row now has an ICM id.
    async fn sync_row(&self, held: Option<&AgentGuard>, row: &MirrorRow) -> Result<Option<bool>> {
        match self.icm_store(&row.scope(), &row.key, &row.content).await {
            Err(_) => Ok(None),
            Ok(None) => Ok(Some(false)),
            Ok(Some(icm_id)) => {
                self.attach_icm_id(held, &row.agent_id, &row.id, &row.content, &icm_id)
                    .await?;
                Ok(Some(true))
            }
        }
    }

    /// Mirror rows that never reached ICM (it was down, or not installed, when
    /// they were stored) are not recalled once their topic holds anything
    /// else, since recall asks ICM first: try a few of them again. Returns how
    /// many now have their copy. Run by the maintenance loop.
    pub async fn backfill_icm(&self) -> Result<u64> {
        // Random order: a row ICM keeps refusing must not hold the others back.
        let rows: Vec<MirrorRow> = sqlx::query_as(&format!(
            "SELECT {MIRROR_ROW_COLUMNS} FROM memories
             WHERE icm_id IS NULL AND trim(content) <> '' AND NOT ({EXPIRED})
             ORDER BY random() LIMIT ?"
        ))
        .bind(ICM_BACKFILL_PER_PASS)
        .fetch_all(&self.db)
        .await?;
        let mut synced = 0u64;
        for row in &rows {
            match self.sync_row(None, row).await? {
                // No `icm` on this host: nothing to retry against.
                None => break,
                Some(done) => synced += u64::from(done),
            }
        }
        Ok(synced)
    }

    /// A store that found its content already held. Nothing is added, but the
    /// call is not ignored: a retention deadline it carries is applied when it
    /// is earlier than the row's (a deadline is only ever brought forward —
    /// nothing is kept longer for having been said twice), and a row that
    /// never reached ICM is given its copy now. The legal basis and the key
    /// stay what they were; the caller reads what is in force in [`Stored`].
    async fn confirm_duplicate(
        &self,
        held: Option<&AgentGuard>,
        dup: Duplicate,
        prov: &Provenance,
    ) -> Result<Stored> {
        let mut retain_until = dup.retain_until;
        if let Some(wanted) = &prov.retain_until {
            // Both went through `normalize_deadline`: lexical is chronological.
            if retain_until.as_ref().is_none_or(|current| wanted < current) {
                sqlx::query(
                    "UPDATE memories SET retain_until = ?1
                     WHERE id = ?2 AND (retain_until IS NULL OR retain_until > ?1)",
                )
                .bind(wanted)
                .bind(&dup.id)
                .execute(&self.db)
                .await?;
                retain_until = Some(wanted.clone());
            }
        }
        if dup.icm_id.is_none() {
            let row: Option<MirrorRow> = sqlx::query_as(&format!(
                "SELECT {MIRROR_ROW_COLUMNS} FROM memories WHERE id = ? AND icm_id IS NULL"
            ))
            .bind(&dup.id)
            .fetch_optional(&self.db)
            .await?;
            if let Some(row) = row {
                self.sync_row(held, &row).await?;
            }
        }
        Ok(Stored {
            id: dup.id,
            created: false,
            retain_until,
            legal_basis: dup.legal_basis,
        })
    }

    /// Persist a memory: DB mirror with provenance + ICM (best-effort).
    ///
    /// Content this scope already holds for the same data subject is not
    /// stored again — no new row; the existing row comes back with
    /// `created: false` (see [`confirm_duplicate`](Self::confirm_duplicate)
    /// for what such a call still changes). The check never crosses scopes,
    /// layers or subjects: two people telling the agent the same thing are two
    /// memories, each erasable on its own.
    pub async fn store_with(
        &self,
        scope: &MemoryScope,
        key: &str,
        content: &str,
        prov: &Provenance,
    ) -> Result<Stored> {
        self.store_inner(None, scope, key, content, prov).await
    }

    /// [`store_with`](Self::store_with) for a caller that already holds the
    /// agent's lock (distillation): the rare steps of a store that erase
    /// something need that lock, and must not wait on their own caller.
    pub async fn store_with_locked(
        &self,
        guard: &AgentGuard,
        scope: &MemoryScope,
        key: &str,
        content: &str,
        prov: &Provenance,
    ) -> Result<Stored> {
        anyhow::ensure!(
            guard.agent_id() == scope.agent_id(),
            "the memory lock held is another agent's"
        );
        self.store_inner(Some(guard), scope, key, content, prov)
            .await
    }

    async fn store_inner(
        &self,
        held: Option<&AgentGuard>,
        scope: &MemoryScope,
        key: &str,
        content: &str,
        prov: &Provenance,
    ) -> Result<Stored> {
        let agent_id = scope.agent_id();
        // For a consumer fork the data subject is that consumer unless the
        // caller says otherwise. Knowledge is nobody's personal data: it is
        // tied to the episodes it came from through `memory_derivations`.
        let subject = match scope {
            MemoryScope::Knowledge { .. } => None,
            _ => prov
                .subject
                .clone()
                .or_else(|| scope.consumer_account().map(str::to_string)),
        };
        let hash = content_hash(content);
        if let Some(dup) = self
            .find_duplicate(scope, subject.as_deref(), &hash)
            .await?
        {
            if dup.expired == 0 {
                return self.confirm_duplicate(held, dup, prov).await;
            }
            // Past its retention and not swept yet: that row is due for
            // erasure, not for a new lease. It goes now, with what was
            // distilled from it, and this store starts afresh.
            let held = self.hold(held, agent_id).await;
            let rows: Vec<(String, Option<String>)> = sqlx::query_as(&format!(
                "SELECT id, icm_id FROM memories WHERE id = ? AND {EXPIRED}"
            ))
            .bind(&dup.id)
            .fetch_all(&self.db)
            .await?;
            self.erase_rows(held.guard(), &rows).await?;
        }

        let source = prov.source.clone().unwrap_or_else(|| match scope {
            MemoryScope::Knowledge { .. } => "distilled".to_string(),
            _ => source_for_key(key).to_string(),
        });
        let id = Uuid::new_v4().to_string();
        // The mirror row first, without its ICM id: a store for an agent that
        // was just deleted fails here, on the foreign key, before anything
        // reaches ICM. `ON CONFLICT DO NOTHING`: two concurrent stores of the
        // same content both pass the check above; the unique index lets one in.
        let inserted = sqlx::query(
            r#"INSERT INTO memories
                 (id, agent_id, consumer_account, layer, key, content, content_hash,
                  source, subject, legal_basis, retain_until, job_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(&id)
        .bind(agent_id)
        .bind(scope.consumer_account())
        .bind(scope.layer())
        .bind(key)
        .bind(content)
        .bind(&hash)
        .bind(&source)
        .bind(&subject)
        .bind(&prov.legal_basis)
        .bind(&prov.retain_until)
        .bind(&prov.job_id)
        .execute(&self.db)
        .await?
        .rows_affected();
        if inserted == 0 {
            // Lost that race: the winner's row is the memory.
            let dup = self
                .find_duplicate(scope, subject.as_deref(), &hash)
                .await?
                .ok_or_else(|| anyhow::anyhow!("memory row vanished during a concurrent store"))?;
            return self.confirm_duplicate(held, dup, prov).await;
        }
        // A row ICM did not take stays in the mirror and is tried again by the
        // maintenance loop (`backfill_icm`).
        if let Ok(Some(icm_id)) = self.icm_store(scope, key, content).await {
            self.attach_icm_id(held, agent_id, &id, content, &icm_id)
                .await?;
        }
        Ok(Stored {
            id,
            created: true,
            retain_until: prov.retain_until.clone(),
            legal_basis: prov.legal_basis.clone(),
        })
    }

    /// Erase every memory of `agent_id` whose data subject is `subject`, in any
    /// scope, on both sides (ICM by id where known, then the mirror). Returns
    /// the number of rows erased. This is the targeted right-to-erasure path;
    /// `forget` (whole scope) is the blunt one.
    pub async fn forget_subject(&self, agent_id: &str, subject: &str) -> Result<Erased> {
        let guard = self.lock_agent(agent_id).await;
        let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, icm_id, consumer_account FROM memories WHERE agent_id = ? AND subject = ?",
        )
        .bind(agent_id)
        .bind(subject)
        .fetch_all(&self.db)
        .await?;
        let doomed: Vec<(String, Option<String>)> = rows
            .iter()
            .map(|(id, icm_id, _)| (id.clone(), icm_id.clone()))
            .collect();
        let mut erased = self.erase_rows(&guard, &doomed).await?;
        // The subject is usually a consumer account: their whole fork topic goes
        // too (exact match), which also covers rows stored before ICM ids were
        // recorded. Only once the fork is empty in the mirror — a row kept there
        // for another subject must keep its ICM copy. Owner-scope rows without
        // an id stay reported as failures.
        let (fork_rows_left,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM memories WHERE agent_id = ? AND consumer_account = ?",
        )
        .bind(agent_id)
        .bind(subject)
        .fetch_one(&self.db)
        .await?;
        if fork_rows_left == 0
            && self
                .forget_topic(&MemoryScope::consumer(agent_id, subject).topic())
                .await
        {
            // Every ICM copy of the fork is gone whatever the per-row outcome.
            let covered = rows
                .iter()
                .filter(|(_, icm, account)| icm.is_none() && account.as_deref() == Some(subject))
                .count() as u64;
            erased.icm_failed = erased.icm_failed.saturating_sub(covered);
        }
        Ok(erased)
    }

    /// Erase one memory row (and its ICM entry) by mirror id, if it belongs to
    /// `agent_id`. Returns whether a row was erased.
    pub async fn forget_one(&self, agent_id: &str, memory_id: &str) -> Result<Option<Erased>> {
        let guard = self.lock_agent(agent_id).await;
        self.forget_one_locked(&guard, memory_id).await
    }

    /// [`forget_one`](Self::forget_one) for a caller that already holds the
    /// agent's lock (the agent is the guard's).
    pub async fn forget_one_locked(
        &self,
        guard: &AgentGuard,
        memory_id: &str,
    ) -> Result<Option<Erased>> {
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, icm_id FROM memories WHERE agent_id = ? AND id = ?")
                .bind(guard.agent_id())
                .bind(memory_id)
                .fetch_all(&self.db)
                .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.erase_rows(guard, &rows).await?))
    }

    /// Erase rows whose retention period has ended, one agent at a time under
    /// its lock. Run by the maintenance loop.
    pub async fn expire_retained(&self) -> Result<Erased> {
        let agents: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT DISTINCT agent_id FROM memories WHERE {EXPIRED}"
        ))
        .fetch_all(&self.db)
        .await?;
        let mut total = Erased::default();
        for agent_id in agents {
            let guard = self.lock_agent(&agent_id).await;
            // Read under the lock: what was due before it may be gone by now.
            let rows: Vec<(String, Option<String>)> = sqlx::query_as(&format!(
                "SELECT id, icm_id FROM memories WHERE agent_id = ? AND {EXPIRED}"
            ))
            .bind(&agent_id)
            .fetch_all(&self.db)
            .await?;
            let erased = self.erase_rows(&guard, &rows).await?;
            total.rows += erased.rows;
            total.derived += erased.derived;
            total.icm_failed += erased.icm_failed;
        }
        Ok(total)
    }

    /// The ICM side of an erasure, under the agent's lock. `leaving` maps each
    /// ICM id carried by the rows on their way out to how many of them carry
    /// it, `gone` holds those rows' mirror ids, and `vanished` adds the
    /// `(icm id, content hash)` of rows the mirror no longer has. Returns how
    /// many leaving rows keep an ICM copy because `icm forget` failed.
    ///
    /// An entry no surviving row points at is forgotten. An entry surviving
    /// rows share is kept as long as it holds nothing but their content (the
    /// same content stored for two subjects is one ICM entry). Otherwise it
    /// holds text of an erased row — with embeddings, ICM folds a
    /// near-duplicate into an existing entry and appends its text — so it is
    /// forgotten and the surviving rows' own content is stored again
    /// ([`icm_fate`]). Forgets run concurrently (bounded).
    async fn release_icm(
        &self,
        _guard: &AgentGuard,
        leaving: &HashMap<String, u64>,
        gone: &HashSet<&str>,
        vanished: &[(String, String)],
    ) -> Result<u64> {
        use futures::stream::{self, StreamExt};
        // Content hashes leaving each entry, and the rows staying on it.
        let mut out: HashMap<String, HashSet<String>> = HashMap::new();
        let mut staying: HashMap<String, Vec<MirrorRow>> = HashMap::new();
        for (icm_id, hash) in vanished {
            out.entry(icm_id.clone()).or_default().insert(hash.clone());
        }
        let icm_ids: Vec<&String> = leaving.keys().collect();
        for chunk in icm_ids.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                "SELECT {MIRROR_ROW_COLUMNS} FROM memories WHERE icm_id IN ({})",
                placeholders(chunk.len())
            );
            let mut q = sqlx::query_as::<_, MirrorRow>(&sql);
            for id in chunk {
                q = q.bind(id.as_str());
            }
            for row in q.fetch_all(&self.db).await? {
                let Some(icm_id) = row.icm_id.clone() else {
                    continue;
                };
                if gone.contains(row.id.as_str()) {
                    out.entry(icm_id)
                        .or_default()
                        .insert(content_hash(&row.content));
                } else {
                    staying.entry(icm_id).or_default().push(row);
                }
            }
        }

        let mut forget: Vec<String> = Vec::new();
        let mut rebuild: Vec<(String, Vec<MirrorRow>)> = Vec::new();
        let nothing = HashSet::new();
        for icm_id in leaving.keys() {
            let rows = staying.remove(icm_id).unwrap_or_default();
            let hashes: Vec<String> = rows.iter().map(|r| content_hash(&r.content)).collect();
            match icm_fate(out.get(icm_id).unwrap_or(&nothing), &hashes) {
                IcmFate::Keep => {}
                IcmFate::Forget => forget.push(icm_id.clone()),
                IcmFate::Rebuild => {
                    forget.push(icm_id.clone());
                    rebuild.push((icm_id.clone(), rows));
                }
            }
        }

        let failed: HashSet<String> = stream::iter(forget)
            .map(|icm_id| {
                let mut cmd = self.icm();
                cmd.arg("forget").arg(&icm_id);
                async move {
                    match cmd.output().await {
                        Ok(o) if o.status.success() => None,
                        Ok(o) => {
                            tracing::warn!(icm_id, stderr = %String::from_utf8_lossy(&o.stderr), "icm forget failed");
                            Some(icm_id)
                        }
                        Err(e) => {
                            tracing::warn!(icm_id, error = %e, "icm forget could not run");
                            Some(icm_id)
                        }
                    }
                }
            })
            .buffer_unordered(8)
            .filter_map(|failed| async move { failed })
            .collect()
            .await;

        // Only once every forget is done: a content stored again must not land
        // in an entry that is itself on its way out.
        for (icm_id, rows) in rebuild {
            if failed.contains(&icm_id) {
                // Still there, and still theirs.
                continue;
            }
            for row in rows {
                let fresh = self
                    .icm_store(&row.scope(), &row.key, &row.content)
                    .await
                    .ok()
                    .flatten();
                if fresh.is_none() {
                    tracing::warn!(row_id = %row.id, "a surviving memory lost its ICM copy; maintenance will store it again");
                }
                sqlx::query("UPDATE memories SET icm_id = ? WHERE id = ?")
                    .bind(&fresh)
                    .bind(&row.id)
                    .execute(&self.db)
                    .await?;
            }
        }
        Ok(failed
            .iter()
            .filter_map(|icm_id| leaving.get(icm_id))
            .sum::<u64>())
    }

    /// Honest erasure: the knowledge distilled from any of `erased` (mirror
    /// ids), and the other episodes that fed that knowledge. Rows already on
    /// their way out are in neither list.
    async fn derived_from(&self, erased: &[(String, Option<String>)]) -> Result<Cascade> {
        let erased_ids: HashSet<&str> = erased.iter().map(|(id, _)| id.as_str()).collect();
        let mut knowledge_ids: Vec<String> = Vec::new();
        for chunk in erased.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                "SELECT DISTINCT knowledge_id FROM memory_derivations
                 WHERE episode_id IN ({})",
                placeholders(chunk.len())
            );
            let mut q = sqlx::query_scalar::<_, String>(&sql);
            for (id, _) in chunk {
                q = q.bind(id);
            }
            knowledge_ids.extend(q.fetch_all(&self.db).await?);
        }
        knowledge_ids.sort();
        knowledge_ids.dedup();
        if knowledge_ids.is_empty() {
            return Ok(Cascade::default());
        }

        let mut cascade = Cascade::default();
        let mut requeue: HashSet<String> = HashSet::new();
        for chunk in knowledge_ids.chunks(SQL_IN_CHUNK) {
            let list = placeholders(chunk.len());
            let sql = format!("SELECT id, icm_id FROM memories WHERE id IN ({list})");
            let mut q = sqlx::query_as::<_, (String, Option<String>)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            cascade.knowledge.extend(
                q.fetch_all(&self.db)
                    .await?
                    .into_iter()
                    .filter(|(id, _)| !erased_ids.contains(id.as_str())),
            );
            let sql = format!(
                "SELECT DISTINCT episode_id FROM memory_derivations
                 WHERE knowledge_id IN ({list})"
            );
            let mut q = sqlx::query_scalar::<_, String>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            requeue.extend(
                q.fetch_all(&self.db)
                    .await?
                    .into_iter()
                    .filter(|id| !erased_ids.contains(id.as_str())),
            );
        }
        cascade.requeue = requeue.into_iter().collect();
        Ok(cascade)
    }

    /// The single way a mirror row leaves: ICM first, then the mirror, under
    /// the agent's lock.
    ///
    /// Erasure is honest about what was derived: every knowledge row distilled
    /// from an erased episode goes with it, and the surviving episodes that fed
    /// that knowledge are queued for distillation again (`distilled_at` NULL),
    /// so it is rebuilt without the erased data. Erasing a knowledge row itself
    /// re-queues nothing — it would only come straight back.
    ///
    /// An ICM entry is forgotten once, unless surviving mirror rows hold the
    /// very same content (see [`release_icm`](Self::release_icm)). Every ICM
    /// failure — unknown id or a failed `icm forget` — is counted per row and
    /// logged: recall consults ICM before the mirror, so a copy left there
    /// would still be served.
    async fn erase_rows(
        &self,
        guard: &AgentGuard,
        rows: &[(String, Option<String>)],
    ) -> Result<Erased> {
        if rows.is_empty() {
            return Ok(Erased::default());
        }
        let cascade = self.derived_from(rows).await?;
        // How many of the rows on their way out point at each ICM entry.
        let mut doomed: HashMap<String, u64> = HashMap::new();
        let mut icm_failed = 0u64;
        for (row_id, icm_id) in rows.iter().chain(&cascade.knowledge) {
            match icm_id {
                Some(icm_id) => *doomed.entry(icm_id.clone()).or_default() += 1,
                None => {
                    tracing::warn!(row_id, "memory has no ICM id; ICM copy (if any) not erased");
                    icm_failed += 1;
                }
            }
        }
        let gone: HashSet<&str> = rows
            .iter()
            .chain(&cascade.knowledge)
            .map(|(id, _)| id.as_str())
            .collect();
        icm_failed += self.release_icm(guard, &doomed, &gone, &[]).await?;

        // The mirror side is one transaction: a knowledge row never outlives
        // the link that says which episodes it came from.
        let mut tx = self.db.begin().await?;
        for chunk in cascade.requeue.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                "UPDATE memories SET distilled_at = NULL WHERE id IN ({})",
                placeholders(chunk.len())
            );
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            q.execute(&mut *tx).await?;
        }
        // [asked for, derived knowledge]
        let mut removed = [0u64; 2];
        for (set, count) in [rows, &cascade.knowledge[..]].into_iter().zip(&mut removed) {
            for chunk in set.chunks(SQL_IN_CHUNK) {
                let list = placeholders(chunk.len());
                for sql in [
                    format!("DELETE FROM memory_derivations WHERE knowledge_id IN ({list})"),
                    format!("DELETE FROM memory_derivations WHERE episode_id IN ({list})"),
                ] {
                    let mut q = sqlx::query(&sql);
                    for (id, _) in chunk {
                        q = q.bind(id);
                    }
                    q.execute(&mut *tx).await?;
                }
                let sql = format!("DELETE FROM memories WHERE id IN ({list})");
                let mut q = sqlx::query(&sql);
                for (id, _) in chunk {
                    q = q.bind(id);
                }
                *count += q.execute(&mut *tx).await?.rows_affected();
            }
        }
        tx.commit().await?;
        Ok(Erased {
            rows: removed[0],
            derived: removed[1],
            icm_failed,
        })
    }

    /// Record a correction (what the agent predicted vs the correct answer) so
    /// the agent improves next time. Backed by ICM feedback, mirrored to DB.
    pub async fn record_feedback(
        &self,
        scope: &MemoryScope,
        subject: Option<&str>,
        context: &str,
        predicted: &str,
        corrected: &str,
        reason: &str,
    ) -> Result<()> {
        let icm = self
            .icm()
            .arg("feedback")
            .arg("record")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--context")
            .arg(context)
            .arg("--predicted")
            .arg(predicted)
            .arg("--corrected")
            .arg(corrected)
            .arg("--reason")
            .arg(reason)
            .arg("--source")
            .arg("user")
            .output()
            .await;
        if let Err(e) = &icm {
            tracing::warn!(topic = %scope.topic(), error = %e, "icm feedback record failed");
        }

        // Mirror as a high-signal memory so it is recalled at the Analyse step.
        let lesson = format!(
            "CORRECTION — when: {context}. Wrong: {predicted}. Correct: {corrected}. Reason: {reason}"
        );
        let mut prov = Provenance::default();
        if let Some(s) = subject {
            prov = prov.subject(s).basis("consent");
        }
        self.store_with(scope, "correction", &lesson, &prov)
            .await
            .map(|_| ())
    }

    /// Recall past corrections relevant to `query` (ICM feedback search).
    pub async fn recall_feedback(&self, scope: &MemoryScope, query: &str, limit: usize) -> String {
        let output = self
            .icm()
            .arg("feedback")
            .arg("search")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--limit")
            .arg(limit.to_string())
            // Free text: after `--` it is never read as a flag.
            .arg("--")
            .arg(query)
            .output()
            .await;
        match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                // When nothing matched, ICM emits an empty TOON header (e.g.
                // `memories[0]{...}:`) that must not be surfaced as fake
                // corrections — treat it as "no feedback".
                if Self::toon_has_entries(&text) {
                    text
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        }
    }

    /// Global ICM statistics (memory count, topics, age).
    pub async fn stats(&self) -> serde_json::Value {
        let out = self.icm().arg("stats").output().await;
        let text = match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => String::new(),
        };
        let mut map = serde_json::Map::new();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once(':') {
                map.insert(
                    k.trim().to_lowercase().replace(' ', "_"),
                    serde_json::json!(v.trim()),
                );
            }
        }
        serde_json::Value::Object(map)
    }

    /// List ICM topics with their memory counts (org-wide memory map).
    pub async fn topics(&self) -> Vec<serde_json::Value> {
        let out = self.icm().arg("topics").output().await;
        let text = match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => return Vec::new(),
        };
        text.lines()
            .filter(|l| l.contains('/')) // topic rows contain the slug path
            .filter_map(|l| {
                let count = l.split_whitespace().last()?.parse::<i64>().ok()?;
                let topic = l.rsplit_once(char::is_whitespace)?.0.trim().to_string();
                Some(serde_json::json!({ "topic": topic, "count": count }))
            })
            .collect()
    }

    /// `icm forget --topic` (ICM matches the topic exactly). Returns whether
    /// ICM confirmed; a failure is logged, the caller decides what it means.
    async fn forget_topic(&self, topic: &str) -> bool {
        match self
            .icm()
            .arg("forget")
            .arg("--topic")
            .arg(topic)
            .output()
            .await
        {
            Ok(o) if o.status.success() => true,
            Ok(o) => {
                tracing::warn!(topic, stderr = %String::from_utf8_lossy(&o.stderr), "icm forget --topic failed");
                false
            }
            Err(e) => {
                tracing::warn!(topic, error = %e, "icm forget --topic could not run");
                false
            }
        }
    }

    /// `icm forget --topic` for each of `topics`, concurrently (bounded).
    /// Returns how many ICM did NOT confirm.
    async fn forget_topics(&self, topics: &[String]) -> u64 {
        use futures::stream::{self, StreamExt};
        // Owned topics: a future borrowing the closure's argument is not `Send`
        // in a way axum accepts.
        stream::iter(topics.to_vec())
            .map(|topic| async move { u64::from(!self.forget_topic(&topic).await) })
            .buffer_unordered(8)
            .fold(0u64, |acc, n| async move { acc + n })
            .await
    }

    /// Purge one scope: its rows in the DB mirror and its ICM topic. An owner
    /// purge leaves every consumer fork in place, and vice versa — but it takes
    /// the knowledge layer along: what was distilled from the owner's episodes
    /// cannot outlive them. Purging the knowledge layer alone leaves the
    /// episodes marked as distilled (it is not rebuilt from them).
    ///
    /// Returns how many ICM topics could NOT be forgotten: their entries are
    /// still recalled (ICM is asked before the mirror), so the caller must
    /// report the purge as incomplete and it must be asked again.
    ///
    /// The mirror goes first: a store in flight then either finds its row
    /// gone and takes its ICM copy back, or lands before the topic is wiped.
    pub async fn forget(&self, scope: &MemoryScope) -> Result<u64> {
        let _guard = self.lock_agent(scope.agent_id()).await;
        let mut scopes = vec![scope.clone()];
        if matches!(scope, MemoryScope::Owner { .. }) {
            scopes.push(MemoryScope::knowledge(scope.agent_id()));
        }
        let mut tx = self.db.begin().await?;
        for scope in &scopes {
            // `memory_derivations` has no foreign key to cascade through.
            for column in ["knowledge_id", "episode_id"] {
                sqlx::query(&format!(
                    "DELETE FROM memory_derivations WHERE {column} IN
                       (SELECT id FROM memories
                        WHERE agent_id = ? AND consumer_account IS ? AND layer = ?)"
                ))
                .bind(scope.agent_id())
                .bind(scope.consumer_account())
                .bind(scope.layer())
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query(
                "DELETE FROM memories WHERE agent_id = ? AND consumer_account IS ? AND layer = ?",
            )
            .bind(scope.agent_id())
            .bind(scope.consumer_account())
            .bind(scope.layer())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        let topics: Vec<String> = scopes.iter().map(MemoryScope::topic).collect();
        Ok(self.forget_topics(&topics).await)
    }

    /// Erase everything an agent remembers, for its deletion: the owner and
    /// knowledge topics, every consumer fork the mirror knows of, then the
    /// mirror rows and their derivation links. The result says how many ICM
    /// topics could NOT be forgotten (their copies outlive the agent), and
    /// lets the caller wipe them once more after deleting the agent row
    /// ([`wipe_again`](Self::wipe_again)).
    pub async fn forget_agent(&self, agent_id: &str) -> Result<AgentWipe> {
        let guard = self.lock_agent(agent_id).await;
        let forks: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT consumer_account FROM memories
             WHERE agent_id = ? AND consumer_account IS NOT NULL",
        )
        .bind(agent_id)
        .fetch_all(&self.db)
        .await?;
        let mut topics = vec![
            MemoryScope::owner(agent_id).topic(),
            MemoryScope::knowledge(agent_id).topic(),
        ];
        topics.extend(
            forks
                .iter()
                .map(|account| MemoryScope::consumer(agent_id, account).topic()),
        );
        let icm_failed = self.forget_topics(&topics).await;

        // `memory_derivations` has no foreign key to cascade through.
        sqlx::query(
            "DELETE FROM memory_derivations
             WHERE knowledge_id IN (SELECT id FROM memories WHERE agent_id = ?1)
                OR episode_id IN (SELECT id FROM memories WHERE agent_id = ?1)",
        )
        .bind(agent_id)
        .execute(&self.db)
        .await?;
        sqlx::query("DELETE FROM memories WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&self.db)
            .await?;
        drop(guard);
        self.locks.release(agent_id);
        Ok(AgentWipe { icm_failed, topics })
    }

    /// Forget a deleted agent's ICM topics a second time, once its row is
    /// gone. A run still in flight during [`forget_agent`](Self::forget_agent)
    /// may have stored into a topic after it was wiped; from the moment the
    /// agent row is deleted no store can succeed any more, so this pass is the
    /// last word. Returns how many topics ICM did not confirm.
    pub async fn wipe_again(&self, wipe: &AgentWipe) -> u64 {
        self.forget_topics(&wipe.topics).await
    }

    /// All of a scope's stored ICM memories with their native importance
    /// metadata (weight, access_count, importance), used to size/color the
    /// memory map.
    ///
    /// Enumerates the topic with `icm list`, NOT `icm recall <keyword>`:
    /// keyword recall only returns entries whose content matches the query
    /// term, so a generic query matches none of the agents' domain content.
    /// `list` returns the whole topic regardless of keywords.
    pub async fn icm_entries(&self, scope: &MemoryScope, limit: usize) -> Vec<IcmEntry> {
        let out = self
            .icm()
            .arg("list")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--format")
            .arg("json")
            .output()
            .await;
        let text = match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => return Vec::new(),
        };
        let parsed: serde_json::Value = match serde_json::from_str(text.trim()) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        parsed
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|m| IcmEntry {
                        id: m.get("id").and_then(|v| v.as_str()).map(str::to_string),
                        summary: m
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        weight: m.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0),
                        access_count: m.get("access_count").and_then(|v| v.as_i64()).unwrap_or(0),
                        importance: m
                            .get("importance")
                            .and_then(|v| v.as_str())
                            .unwrap_or("medium")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Agent ids whose OWNER memory holds at least `min` episodes.
    pub async fn agents_with_memory(&self, min: i64) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT agent_id FROM memories
             WHERE consumer_account IS NULL AND layer = 'episode'
             GROUP BY agent_id HAVING COUNT(*) >= ?",
        )
        .bind(min)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }

    /// List stored memories of one scope (UI / consumer API).
    pub async fn list(&self, scope: &MemoryScope) -> Result<Vec<MemoryEntry>> {
        let rows = sqlx::query_as::<_, MemoryEntry>(&format!(
            "SELECT {ENTRY_COLUMNS} FROM memories
             WHERE agent_id = ? AND consumer_account IS ? AND layer = ?
             ORDER BY created_at DESC"
        ))
        .bind(scope.agent_id())
        .bind(scope.consumer_account())
        .bind(scope.layer())
        .fetch_all(&self.db)
        .await?;
        Ok(rows)
    }

    /// The retention half of a maintenance pass. It only erases what is past
    /// its retention: episodes are never summarised away, decayed or pruned
    /// here. (ICM still decays weights on its own, at most once a day at
    /// recall, which reorders results and deletes nothing.)
    pub async fn maintain(&self) {
        match self.expire_retained().await {
            Ok(e) if e.rows > 0 => tracing::info!(
                erased = e.rows,
                derived = e.derived,
                icm_failed = e.icm_failed,
                "erased memories past their retention"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "retention sweep failed"),
        }
    }

    /// The upkeep half of a maintenance pass: what ICM missed is stored there
    /// again ([`backfill_icm`](Self::backfill_icm)).
    pub async fn upkeep(&self) {
        match self.backfill_icm().await {
            Ok(0) => {}
            Ok(synced) => tracing::info!(synced, "memories stored again in ICM"),
            Err(e) => tracing::warn!(error = %e, "ICM backfill failed"),
        }
    }
}

/// One recalled memory on its way into a prompt, whichever source served it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recalled {
    /// ICM's id, so an entry is not shown twice across blocks.
    icm_id: Option<String>,
    importance: String,
    summary: String,
}

impl Recalled {
    /// The one shape a memory takes in a prompt.
    fn line(&self) -> String {
        format!("- [{}] {}", self.importance, self.summary)
    }
}

/// The entries of an `icm recall` / `icm list` JSON payload (`--format json`)
/// whose `topic` is exactly `wanted`, in ICM's order. Rows without a summary
/// are skipped; anything that is not a JSON list yields nothing.
fn parse_recalled(json: &str, wanted: &str) -> Vec<Recalled> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json.trim()).unwrap_or_default();
    rows.iter()
        .filter(|r| r.get("topic").and_then(|t| t.as_str()) == Some(wanted))
        .filter_map(|r| {
            let summary = r.get("summary").and_then(|s| s.as_str())?.trim();
            if summary.is_empty() {
                return None;
            }
            Some(Recalled {
                icm_id: r
                    .get("id")
                    .and_then(|i| i.as_str())
                    .filter(|i| !i.is_empty())
                    .map(str::to_string),
                importance: r
                    .get("importance")
                    .and_then(|i| i.as_str())
                    .unwrap_or("medium")
                    .to_string(),
                summary: summary.to_string(),
            })
        })
        .collect()
}

/// Drop the entries already in `shown` — same ICM id, or same content up to
/// spacing and case — and record the ones kept, so neither a later block nor
/// the rest of this one repeats them.
fn distinct(entries: Vec<Recalled>, shown: &mut HashSet<String>) -> Vec<Recalled> {
    entries
        .into_iter()
        .filter(|e| {
            let hash = content_hash(&e.summary);
            if shown.contains(&hash) || e.icm_id.as_ref().is_some_and(|id| shown.contains(id)) {
                return false;
            }
            shown.insert(hash);
            if let Some(id) = &e.icm_id {
                shown.insert(id.clone());
            }
            true
        })
        .collect()
}

/// Spend `budget` chars on whole entries, in order (the rendered lines and the
/// newlines joining them are what is counted). An entry that does not fit in
/// what is left is dropped, and the next ones are still tried: a shorter one
/// may fit.
///
/// An entry is cut in one case only: nothing fits whole, so the block would be
/// empty. The best-ranked entry is then shown alone, cut on a char boundary
/// and ending with an ellipsis.
fn within_budget(entries: Vec<Recalled>, budget: usize) -> Vec<Recalled> {
    let mut kept: Vec<Recalled> = Vec::new();
    let mut left = budget;
    let mut best: Option<Recalled> = None;
    for (rank, entry) in entries.into_iter().enumerate() {
        let joint = usize::from(!kept.is_empty());
        let len = entry.line().chars().count();
        if joint + len <= left {
            left -= joint + len;
            kept.push(entry);
        } else if rank == 0 {
            best = Some(entry);
        }
    }
    if kept.is_empty() {
        kept.extend(best.and_then(|entry| cut_to(entry, budget)));
    }
    kept
}

/// `entry` with its summary cut so that its line takes at most `budget` chars,
/// ellipsis included. `None` when not even one char of the summary fits.
fn cut_to(mut entry: Recalled, budget: usize) -> Option<Recalled> {
    let summary_chars = entry.summary.chars().count();
    // What `line()` puts before the summary: `- [importance] `.
    let prefix = entry.line().chars().count() - summary_chars;
    let room = budget.saturating_sub(prefix + 1).min(summary_chars);
    if room == 0 {
        return None;
    }
    let cut: String = entry.summary.chars().take(room).collect();
    entry.summary = format!("{}…", cut.trim_end());
    Some(entry)
}

/// Entries as prompt lines, one `- [importance] summary` each.
fn render_entries(entries: &[Recalled]) -> String {
    entries
        .iter()
        .map(Recalled::line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Join the non-empty `(heading, body)` blocks of a recall, in order.
fn compose_blocks(blocks: &[(&str, String)]) -> String {
    blocks
        .iter()
        .filter(|(_, body)| !body.trim().is_empty())
        .map(|(heading, body)| format!("{heading}\n{body}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `?,?,…` for an `IN (…)` list of `n` bound values.
fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// What becomes of one ICM entry when mirror rows pointing at it are erased,
/// from the content hashes of the rows `leaving` it and of those `staying`.
/// It may stay only if every text that leaves is also a text that stays.
fn icm_fate(leaving: &HashSet<String>, staying: &[String]) -> IcmFate {
    if staying.is_empty() {
        IcmFate::Forget
    } else if leaving.iter().all(|hash| staying.contains(hash)) {
        IcmFate::Keep
    } else {
        IcmFate::Rebuild
    }
}

/// Spawn the recurring background memory maintenance.
///
/// The main loop erases what is past its retention ([`Memory::maintain`]),
/// distils the episodes that are waiting into knowledge
/// ([`crate::distill::run_pass`]) — in that order, so a pass starts from a
/// swept memory — then stores again what ICM missed ([`Memory::upkeep`]).
///
/// A distillation pass waits on a model once per agent and can last a long
/// time, so the retention sweep also runs on its own, on the same interval:
/// a deadline is honoured whatever the pass is doing. (Distillation never
/// reads a row past its deadline either way.)
pub fn spawn_maintenance(state: crate::state::AppState, interval_secs: u64) {
    use std::time::Duration;
    let interval = Duration::from_secs(interval_secs);
    // Let the server settle before the first pass, but never wait longer than
    // one full interval (so a short demo cadence starts quickly).
    let settle = Duration::from_secs(interval_secs.min(120));
    tracing::info!(interval_secs, "memory maintenance loop started");
    let memory = state.memory.clone();
    tokio::spawn(async move {
        tokio::time::sleep(settle + interval / 2).await;
        loop {
            memory.maintain().await;
            tokio::time::sleep(interval).await;
        }
    });
    tokio::spawn(async move {
        let distiller = crate::distill::ProviderDistiller::new(state.clone());
        let mut backoff = crate::distill::Backoff::default();
        tokio::time::sleep(settle).await;
        loop {
            state.memory.maintain().await;
            crate::distill::run_pass(&state.memory, &distiller, &mut backoff).await;
            state.memory.upkeep().await;
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_round_trips_for_both_scopes() {
        let o = MemoryScope::owner("agent-1");
        assert_eq!(o.topic(), "takoia/agent/agent-1");
        assert_eq!(MemoryScope::parse_topic(&o.topic()), Some(o.clone()));
        assert_eq!(o.consumer_account(), None);
        let c = MemoryScope::consumer("agent-1", "acct-b");
        assert_eq!(c.topic(), "takoia/fork/agent-1/acct-b");
        assert!(
            !c.topic().starts_with(&o.topic()),
            "a fork topic must never extend the owner topic (icm recall is a prefix match)"
        );
        assert_eq!(MemoryScope::parse_topic(&c.topic()), Some(c.clone()));
        assert_eq!(c.consumer_account(), Some("acct-b"));
        assert_eq!(c.agent_id(), "agent-1");
    }

    #[test]
    fn parse_rejects_malformed_topics() {
        for t in [
            "takoia/agent/",
            "other/agent/x",
            "takoia/fork/a/",
            "takoia/fork//b",
            "takoia/fork/a",
            "takoia/agent/a/b",
            "takoia/fork/a/b/c",
            "",
        ] {
            assert!(
                MemoryScope::parse_topic(t).is_none(),
                "{t:?} must not parse"
            );
        }
    }

    fn entry(id: Option<&str>, importance: &str, summary: &str) -> Recalled {
        Recalled {
            icm_id: id.map(str::to_string),
            importance: importance.to_string(),
            summary: summary.to_string(),
        }
    }

    #[test]
    fn recalled_entries_keep_only_the_exact_topic() {
        let json = r#"[
          {"id":"I1","topic":"takoia/agent/a1","summary":" owner fact ","importance":"high"},
          {"id":"I2","topic":"takoia/agent/a10","summary":"sibling fact","importance":"medium"},
          {"id":"I3","topic":"takoia/fork/a1/c1","summary":"consumer fact","importance":"low"},
          {"id":"I4","topic":"takoia/agent/a1","summary":"   ","importance":"low"},
          {"topic":"takoia/agent/a1","summary":"no id, no importance"},
          {"id":"I6","topic":"takoia/agent/a1"}
        ]"#;
        assert_eq!(
            parse_recalled(json, "takoia/agent/a1"),
            [
                entry(Some("I1"), "high", "owner fact"),
                entry(None, "medium", "no id, no importance"),
            ]
        );
        assert_eq!(
            parse_recalled(json, "takoia/fork/a1/c1"),
            [entry(Some("I3"), "low", "consumer fact")]
        );
        assert!(parse_recalled(json, "takoia/agent/a2").is_empty());
        // Not a JSON list: a TOON header, a human sentinel, an object, nothing.
        for other in [
            "memories[0]{id}:",
            "No memories found.",
            r#"{"topic":"takoia/agent/a1","summary":"x"}"#,
            "[]",
            "",
        ] {
            assert!(
                parse_recalled(other, "takoia/agent/a1").is_empty(),
                "{other:?}"
            );
        }
    }

    #[test]
    fn every_recalled_memory_renders_as_one_importance_line() {
        assert_eq!(
            render_entries(&[
                entry(Some("I1"), "high", "first"),
                entry(None, "low", "second")
            ]),
            "- [high] first\n- [low] second"
        );
        assert_eq!(render_entries(&[]), "");
    }

    #[test]
    fn importance_follows_the_layer_then_the_key() {
        assert_eq!(importance_of(LAYER_KNOWLEDGE, "fact"), Some("high"));
        assert_eq!(importance_of(LAYER_KNOWLEDGE, "run-summary"), Some("high"));
        assert_eq!(importance_of(LAYER_EPISODE, "correction"), Some("high"));
        assert_eq!(importance_of(LAYER_EPISODE, "instruction"), Some("high"));
        assert_eq!(importance_of(LAYER_EPISODE, "run-summary"), Some("low"));
        assert_eq!(importance_of(LAYER_EPISODE, "reflection"), Some("low"));
        assert_eq!(importance_of(LAYER_EPISODE, "analyse"), None);
    }

    #[test]
    fn an_entry_is_shown_once_across_blocks() {
        let mut shown = HashSet::new();
        let first = distinct(
            vec![
                entry(Some("I1"), "high", "Check the totals"),
                entry(Some("I2"), "high", "Answer briefly"),
                // Same ICM entry served twice.
                entry(Some("I1"), "high", "Check the totals"),
            ],
            &mut shown,
        );
        assert_eq!(first.len(), 2);
        let second = distinct(
            vec![
                // Same ICM id, whatever the text.
                entry(Some("I2"), "medium", "reworded"),
                // Same content from another source: no id, other spacing/case.
                entry(None, "medium", "  check   the TOTALS "),
                entry(Some("I9"), "medium", "check the totals"),
                entry(Some("I3"), "medium", "Something new"),
                entry(None, "low", "No id, new content"),
            ],
            &mut shown,
        );
        assert_eq!(
            second,
            [
                entry(Some("I3"), "medium", "Something new"),
                entry(None, "low", "No id, new content"),
            ]
        );
        // An entry that was never shown hides nothing.
        assert_eq!(
            distinct(
                vec![entry(Some("I9"), "low", "other text")],
                &mut HashSet::new()
            )
            .len(),
            1
        );
    }

    #[test]
    fn a_budget_is_spent_on_whole_entries() {
        let chars = |kept: &[Recalled]| render_entries(kept).chars().count();
        // "- [high] " is 9 chars: these lines are 109, 309 and 59 chars long.
        let a = entry(Some("A"), "high", &"a".repeat(100));
        let b = entry(Some("B"), "high", &"b".repeat(300));
        let c = entry(Some("C"), "high", &"c".repeat(50));
        let all = vec![a.clone(), b.clone(), c.clone()];

        // Everything fits: untouched, in order (two joining newlines counted).
        assert_eq!(within_budget(all.clone(), 479), all);
        assert_eq!(chars(&all), 479);
        // One char short: the last entry goes, whole.
        assert_eq!(within_budget(all.clone(), 478), [a.clone(), b.clone()]);
        // An entry that does not fit is dropped, a shorter one after it kept.
        assert_eq!(within_budget(all.clone(), 300), [a.clone(), c.clone()]);
        assert_eq!(within_budget(all.clone(), 109), std::slice::from_ref(&a));
        assert_eq!(within_budget(all.clone(), 108), std::slice::from_ref(&c));
        assert!(within_budget(Vec::new(), 2000).is_empty());

        // Whatever the budget: never over it, and every line is a whole entry —
        // or the block is ONE entry, the best-ranked, cut with an ellipsis.
        let long = entry(Some("L"), "medium", &"é日本 🙂 ".repeat(400));
        let ranked = vec![long.clone(), a.clone(), b.clone(), c.clone()];
        let whole: Vec<String> = ranked.iter().map(Recalled::line).collect();
        let mut cuts = 0;
        for budget in 0..2600 {
            let kept = within_budget(ranked.clone(), budget);
            assert!(chars(&kept) <= budget, "budget {budget} overspent");
            if kept.iter().all(|e| whole.contains(&e.line())) {
                continue;
            }
            cuts += 1;
            assert_eq!(kept.len(), 1, "budget {budget}: a cut entry is alone");
            assert!(budget < 59, "budget {budget}: cut although an entry fits");
            let line = kept[0].line();
            let body = line.strip_suffix('…').expect("a cut entry ends with …");
            assert!(long.line().starts_with(body), "budget {budget}");
            assert!(!body.ends_with(char::is_whitespace), "budget {budget}");
        }
        // "- [medium] " is 11 chars: from 13 on there is room for one and "…".
        assert_eq!(cuts, 59 - 13);
        assert!(within_budget(ranked.clone(), 12).is_empty());
    }

    #[test]
    fn an_entry_is_cut_only_when_nothing_fits_whole() {
        let long = entry(Some("L"), "low", &"é".repeat(5000));
        let longer = entry(Some("M"), "low", &"ü".repeat(6000));
        let short = entry(Some("S"), "high", "short one");

        // Alone: cut to exactly the budget, on a char boundary, with an ellipsis.
        let kept = within_budget(vec![long.clone()], 2000);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].line(), format!("- [low] {}…", "é".repeat(1991)));
        assert_eq!(kept[0].line().chars().count(), 2000);
        assert_eq!(kept[0].icm_id.as_deref(), Some("L"));
        // Several that cannot fit: the best-ranked one, alone.
        let kept = within_budget(vec![longer.clone(), long.clone()], 2000);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].line(), format!("- [low] {}…", "ü".repeat(1991)));

        // As soon as one entry fits whole, nothing is cut — whatever its rank.
        assert_eq!(
            within_budget(vec![long.clone(), short.clone(), longer.clone()], 2000),
            std::slice::from_ref(&short)
        );
        assert_eq!(
            within_budget(vec![short.clone(), long.clone()], 2000),
            std::slice::from_ref(&short)
        );

        // The cut never ends on spacing, and an entry is not padded to be cut.
        let spaced = entry(
            None,
            "low",
            &format!("{}   {}", "a".repeat(10), "b".repeat(50)),
        );
        assert_eq!(
            within_budget(vec![spaced], 21)[0].line(),
            format!("- [low] {}…", "a".repeat(10))
        );
        assert_eq!(
            cut_to(short.clone(), 2000).map(|e| e.summary),
            Some("short one…".to_string())
        );
        assert_eq!(cut_to(short.clone(), 10), None);
        assert_eq!(
            cut_to(short, 11).map(|e| e.line()),
            Some("- [high] s…".into())
        );
    }

    #[test]
    fn composed_recall_puts_the_personal_part_last() {
        let block = |body: &str| body.to_string();
        assert_eq!(compose_blocks(&[("K:", block("")), ("P:", block(" "))]), "");
        assert_eq!(
            compose_blocks(&[("K:", block("- [high] rule")), ("P:", block(""))]),
            "K:\n- [high] rule"
        );
        assert_eq!(
            compose_blocks(&[("K:", block("")), ("P:", block("- [low] likes brevity"))]),
            "P:\n- [low] likes brevity"
        );
        assert_eq!(
            compose_blocks(&[
                ("K:", block("- [high] rule")),
                ("P:", block("- [low] likes brevity"))
            ]),
            "K:\n- [high] rule\n\nP:\n- [low] likes brevity"
        );
    }

    #[test]
    fn icm_stored_id_is_parsed_from_the_cli_line() {
        assert_eq!(
            parse_icm_stored_id("Stored: 01M3G4T5CM634F1X1RYBEGPMGJ (+5 links)\n").as_deref(),
            Some("01M3G4T5CM634F1X1RYBEGPMGJ")
        );
        assert_eq!(parse_icm_stored_id("Stored: \n"), None);
        assert_eq!(parse_icm_stored_id("No memories found."), None);
        // An exact duplicate prints the existing id on the same line shape.
        assert_eq!(
            parse_icm_stored_id("Stored: 01M45402421DWYAD1R73VNK5KR\n").as_deref(),
            Some("01M45402421DWYAD1R73VNK5KR")
        );
        // With embeddings, a near-duplicate is folded into the existing row.
        assert_eq!(
            parse_icm_stored_id(
                "Updated existing memory (similarity 0.98): 01M3G4T5CM634F1X1RYBEGPMGJ\n"
            )
            .as_deref(),
            Some("01M3G4T5CM634F1X1RYBEGPMGJ")
        );
        assert_eq!(
            parse_icm_stored_id("Updated existing memory (similarity 0.98): "),
            None
        );
        assert_eq!(parse_icm_stored_id("Updated existing memory"), None);
    }

    #[test]
    fn deadlines_are_normalised_to_utc_millis_or_rejected() {
        assert_eq!(
            normalize_deadline("2026-09-27T01:00:00+02:00").unwrap(),
            "2026-09-26T23:00:00.000Z"
        );
        assert_eq!(
            normalize_deadline("2027-01-01T00:00:00Z").unwrap(),
            "2027-01-01T00:00:00.000Z"
        );
        for bad in ["1 year", "12/31/2027", "tomorrow", "2026-09-27", ""] {
            assert!(normalize_deadline(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn knowledge_topic_round_trips_and_is_no_substring_of_the_episode_topics() {
        let k = MemoryScope::knowledge("agent-1");
        assert_eq!(k.topic(), "takoia/know/agent-1");
        assert_eq!(MemoryScope::parse_topic(&k.topic()), Some(k.clone()));
        assert_eq!(k.agent_id(), "agent-1");
        assert_eq!(k.consumer_account(), None);
        assert_eq!(k.layer(), LAYER_KNOWLEDGE);
        assert_eq!(MemoryScope::owner("agent-1").layer(), LAYER_EPISODE);
        assert_eq!(MemoryScope::consumer("agent-1", "b").layer(), LAYER_EPISODE);
        // `icm recall --topic` matches by substring in both directions.
        for episode in [
            MemoryScope::owner("agent-1").topic(),
            MemoryScope::consumer("agent-1", "acct-b").topic(),
        ] {
            assert!(!episode.contains(&k.topic()) && !k.topic().contains(&episode));
        }
        for t in ["takoia/know/", "takoia/know/a/b", "takoia/knowledge/a"] {
            assert!(
                MemoryScope::parse_topic(t).is_none(),
                "{t:?} must not parse"
            );
        }
    }

    #[test]
    fn content_hash_ignores_spacing_and_case_only() {
        let h = content_hash("Client prefers French");
        assert_eq!(h.len(), 64);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Known vector: SHA-256 of "abc".
        assert_eq!(
            content_hash("  ABC\n"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(h, content_hash("  client   PREFERS\n\tfrench  "));
        assert_ne!(h, content_hash("Client prefers German"));
        assert_ne!(h, content_hash("Clientprefers French"));
        assert_ne!(h, content_hash("Client prefers French."));
    }

    #[test]
    fn an_icm_entry_stays_only_while_it_holds_nothing_but_surviving_content() {
        let set =
            |hashes: &[&str]| -> HashSet<String> { hashes.iter().map(|h| h.to_string()).collect() };
        let list =
            |hashes: &[&str]| -> Vec<String> { hashes.iter().map(|h| h.to_string()).collect() };
        // No row left on it.
        assert_eq!(icm_fate(&set(&["a"]), &[]), IcmFate::Forget);
        assert_eq!(icm_fate(&set(&["a", "b"]), &[]), IcmFate::Forget);
        // The same content stored for two subjects, one of them erased.
        assert_eq!(icm_fate(&set(&["a"]), &list(&["a"])), IcmFate::Keep);
        assert_eq!(icm_fate(&set(&["a"]), &list(&["a", "a"])), IcmFate::Keep);
        // A merged entry: the row leaving has text the rows staying do not.
        // Keeping it would leave erased text in ICM, reported as erased.
        assert_eq!(icm_fate(&set(&["a"]), &list(&["b"])), IcmFate::Rebuild);
        assert_eq!(icm_fate(&set(&["a", "b"]), &list(&["b"])), IcmFate::Rebuild);
        assert_eq!(icm_fate(&set(&["c"]), &list(&["a", "b"])), IcmFate::Rebuild);
        // Merged, but everything that leaves is also held by a row that stays.
        assert_eq!(icm_fate(&set(&["a"]), &list(&["a", "b"])), IcmFate::Keep);
        // Nothing known to leave (the row was already gone): left alone.
        assert_eq!(icm_fate(&set(&[]), &list(&["a"])), IcmFate::Keep);
    }

    #[test]
    fn a_mirror_row_knows_its_scope() {
        let row = |account: Option<&str>, layer: &str| MirrorRow {
            id: "m".into(),
            agent_id: "ag".into(),
            consumer_account: account.map(str::to_string),
            layer: layer.into(),
            key: "rule".into(),
            content: "c".into(),
            icm_id: None,
        };
        assert_eq!(row(None, LAYER_EPISODE).scope(), MemoryScope::owner("ag"));
        assert_eq!(
            row(None, LAYER_KNOWLEDGE).scope(),
            MemoryScope::knowledge("ag")
        );
        assert_eq!(
            row(Some("acct"), LAYER_EPISODE).scope(),
            MemoryScope::consumer("ag", "acct")
        );
    }

    #[tokio::test]
    async fn every_icm_command_targets_the_dedicated_db_and_embeddings_are_opt_in() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let args = |m: &Memory| -> Vec<String> {
            let cmd = m.icm();
            assert_eq!(cmd.as_std().get_program(), "icm");
            cmd.as_std()
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let memory = Memory::new(pool, "/tmp/x/icm.db".into());
        assert_eq!(args(&memory), ["--db", "/tmp/x/icm.db", "--no-embeddings"]);
        assert_eq!(
            args(&memory.clone().with_embeddings(true)),
            ["--db", "/tmp/x/icm.db"]
        );
    }

    #[tokio::test]
    async fn the_agent_lock_is_exclusive_per_agent_and_shared_by_clones() {
        use std::time::Duration;
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let memory = Memory::new(pool, "unused".into());
        let clone = memory.clone();
        let held = memory.lock_agent("a").await;
        assert_eq!(held.agent_id(), "a");
        // Same agent, through a clone: must wait. Another agent: immediate.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), clone.lock_agent("a"))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), clone.lock_agent("b"))
                .await
                .is_ok()
        );
        drop(held);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), clone.lock_agent("a"))
                .await
                .is_ok()
        );
        // A released agent's entry leaves the map; a held one does not.
        let held = memory.lock_agent("a").await;
        memory.locks.release("a");
        memory.locks.release("b");
        let len = |m: &Memory| m.locks.0.lock().unwrap().len();
        assert_eq!(len(&memory), 1, "held lock kept, idle one dropped");
        drop(held);
        memory.locks.release("a");
        assert_eq!(len(&memory), 0);
    }

    #[test]
    fn source_defaults_follow_the_key() {
        assert_eq!(source_for_key("run-summary"), "run");
        assert_eq!(source_for_key("interaction"), "interaction");
        assert_eq!(source_for_key("analyse"), "step");
        assert_eq!(source_for_key("preference"), "manual");
        assert_eq!(source_for_key("whatever"), "run");
    }
}
