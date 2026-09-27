//! Permanent agent memory — the "personalize your AI over time" capability.
//!
//! Backed by ICM (`icm` CLI) for semantic recall and consolidation, with the
//! `memories` table as the always-available source of truth for the UI and as a
//! fallback if ICM is unavailable. Each agent owns an ICM topic, so an expert
//! agent (e.g. trading) accumulates and refines expertise across runs.
//!
//! Memory is scoped ([`MemoryScope`]): the owner's curated memory lives in
//! `takoia/agent/{id}`; every marketplace consumer gets a fork of their own in
//! `takoia/agent/{id}/consumer/{account}`. A consumer run recalls both (owner
//! first, read-only) and writes only to its fork, so the agent becomes theirs
//! without their inputs ever reaching the publisher's expertise.

use crate::db::Db;
use anyhow::Result;
use serde::Serialize;
use tokio::process::Command;
use uuid::Uuid;

/// An ICM memory with its native importance metadata.
#[derive(Debug, Clone, Serialize)]
pub struct IcmEntry {
    /// ICM's own id, so a mirrored (or consolidated) row can be erased there.
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
            MemoryScope::Owner { .. } => p,
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
    pub rows: u64,
    pub icm_failed: u64,
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

/// The id ICM printed for a stored memory (`Stored: <id> (+N links)`).
fn parse_icm_stored_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("Stored:"))
        .and_then(|rest| rest.split_whitespace().next())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Whose memory a read or write addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryScope {
    /// The publisher's own memory of the agent: `takoia/agent/{id}`.
    Owner { agent_id: String },
    /// One consumer account's fork: `takoia/agent/{id}/consumer/{account}`.
    Consumer {
        agent_id: String,
        account_id: String,
    },
}

const TOPIC_PREFIX: &str = "takoia/agent/";
/// Forks live under a DIFFERENT root on purpose: `icm recall --topic` matches
/// topics by prefix (verified against icm 0.10.63), so a fork named
/// `takoia/agent/{id}/...` would be recalled by the owner's runs and by every
/// other consumer's. `takoia/fork/{agent}/{account}` shares no prefix with
/// `takoia/agent/{agent}`.
const FORK_PREFIX: &str = "takoia/fork/";

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

    pub fn agent_id(&self) -> &str {
        match self {
            MemoryScope::Owner { agent_id } | MemoryScope::Consumer { agent_id, .. } => agent_id,
        }
    }

    /// `None` for the owner's memory, the account id for a consumer fork. Also
    /// the value of `memories.consumer_account`.
    pub fn consumer_account(&self) -> Option<&str> {
        match self {
            MemoryScope::Owner { .. } => None,
            MemoryScope::Consumer { account_id, .. } => Some(account_id),
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
        }
    }

    /// Inverse of [`topic`](Self::topic). Rejects anything that is not exactly
    /// one of the two shapes (a bare `strip_prefix` would read a consumer topic
    /// as an owner topic with a bogus agent id).
    pub fn parse_topic(topic: &str) -> Option<MemoryScope> {
        if let Some(agent) = topic.strip_prefix(TOPIC_PREFIX) {
            return (!agent.is_empty() && !agent.contains('/')).then(|| MemoryScope::owner(agent));
        }
        let rest = topic.strip_prefix(FORK_PREFIX)?;
        let (agent, account) = rest.split_once('/')?;
        (!agent.is_empty() && !account.is_empty() && !account.contains('/'))
            .then(|| MemoryScope::consumer(agent, account))
    }
}

/// Memory store bridging ICM and the local `memories` table.
#[derive(Clone)]
pub struct Memory {
    db: Db,
    icm_db_path: String,
}

/// Prompt budget for the owner's recalled memory.
const OWNER_RECALL_CHARS: usize = 4000;
/// Prompt budget for a consumer's own fork, appended after the owner's memory.
const CONSUMER_RECALL_CHARS: usize = 2000;

impl Memory {
    pub fn new(db: Db, icm_db_path: String) -> Self {
        Self { db, icm_db_path }
    }

    /// What a marketplace consumer's run recalls: the publisher's curated memory
    /// first (the expertise they pay for), then the consumer's own fork (what
    /// the agent has learnt about THEM), each within its own budget. The
    /// personal part comes last so it is the freshest context before the task.
    pub async fn recall_composed(
        &self,
        agent_id: &str,
        consumer_account: Option<&str>,
        query: &str,
        limit: usize,
    ) -> String {
        let owner_scope = MemoryScope::owner(agent_id);
        let Some(account) = consumer_account else {
            return self
                .recall(&owner_scope, query, limit)
                .await
                .chars()
                .take(OWNER_RECALL_CHARS)
                .collect();
        };
        // Independent lookups (each up to three `icm` spawns): run them together.
        let fork_scope = MemoryScope::consumer(agent_id, account);
        let (owner, own) = tokio::join!(
            self.recall(&owner_scope, query, limit),
            self.recall(&fork_scope, query, limit)
        );
        let owner: String = owner.chars().take(OWNER_RECALL_CHARS).collect();
        let own: String = own.chars().take(CONSUMER_RECALL_CHARS).collect();
        compose_recall(&owner, &own)
    }

    /// Recall expertise relevant to `query` for prompt injection at the Analyse
    /// step. Tries ICM first (semantic), falls back to recent DB memories.
    pub async fn recall(&self, scope: &MemoryScope, query: &str, limit: usize) -> String {
        // 1) Query-scoped keyword recall (exact topic, see `recall_icm`).
        if let Some(text) = self.recall_icm(scope, query, limit).await {
            return text;
        }
        // 2) Keyword recall frequently misses (memories carry generic step-name
        //    keywords, not content terms), in which case ICM returns an empty
        //    `memories[0]{...}` header. Fall back to the agent's highest-weight
        //    memories so accumulated expertise is ALWAYS injected.
        if let Some(text) = self.recall_top(scope).await {
            if !text.trim().is_empty() {
                return text;
            }
        }
        // 3) Last resort: the DB mirror.
        self.recall_db(scope, limit).await.unwrap_or_default()
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

    /// Highest-weight memories for the agent's topic, independent of the query.
    /// `icm recall` needs a keyword/embedding match; `icm list` does not, so this
    /// reliably surfaces the consolidated expertise even when recall misses.
    async fn recall_top(&self, scope: &MemoryScope) -> Option<String> {
        let output = Command::new("icm")
            .arg("list")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--sort")
            .arg("weight")
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // `icm list` prints a human sentinel (e.g. "No memories found.") with a
        // success exit code when the topic is empty. Only treat the output as
        // real memory when it carries actual entry fields, so we never inject a
        // sentinel string as recalled memory (and never shadow the DB fallback).
        let looks_real = text.contains("summary:") || text.contains("topic:");
        if text.is_empty() || !looks_real {
            return None;
        }
        // Cap what we inject into the prompt regardless of how much ICM holds.
        Some(text.chars().take(4000).collect())
    }

    /// Query-scoped recall, filtered to EXACTLY this scope's topic. `icm recall
    /// --topic` is a prefix filter, so without this a slug agent id would pull
    /// in its longer siblings (`invoice-bot` ⊂ `invoice-bot-v2`). Returns
    /// `None` when nothing (of this topic) matched.
    async fn recall_icm(&self, scope: &MemoryScope, query: &str, limit: usize) -> Option<String> {
        let topic = scope.topic();
        let output = Command::new("icm")
            .arg("recall")
            .arg(query)
            .arg("--topic")
            .arg(&topic)
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--format")
            .arg("json")
            // Keyword search, matching how we store (no embedding model download).
            .arg("--no-embeddings")
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            tracing::warn!(%topic, "icm recall failed, falling back to db memory");
            return None;
        }
        render_recall(&String::from_utf8_lossy(&output.stdout), &topic)
    }

    async fn recall_db(&self, scope: &MemoryScope, limit: usize) -> Result<String> {
        // `IS ?` so a NULL bind matches the owner rows (`= NULL` never matches).
        let rows = sqlx::query_as::<_, MemoryEntry>(
            r#"SELECT id, agent_id, key, content, created_at,
                      source, subject, legal_basis, retain_until, job_id
               FROM memories WHERE agent_id = ? AND consumer_account IS ?
               ORDER BY created_at DESC LIMIT ?"#,
        )
        .bind(scope.agent_id())
        .bind(scope.consumer_account())
        .bind(limit as i64)
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .map(|m| format!("- [{}] {}", m.key, m.content))
            .collect::<Vec<_>>()
            .join("\n"))
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
    }

    /// Persist a new memory: ICM (best-effort) + DB mirror with provenance.
    pub async fn store_with(
        &self,
        scope: &MemoryScope,
        key: &str,
        content: &str,
        prov: &Provenance,
    ) -> Result<()> {
        let agent_id = scope.agent_id();
        // User-specific memories are protected from decay/consolidation by
        // storing them at high importance; other (generic step) memories keep
        // ICM's default (medium).
        // User-authored signal outranks the agent's own output: corrections and
        // preferences must survive consolidation and decay; run summaries and
        // self-reflections are the bulk that decay should thin out first.
        let importance = match key {
            "correction" | "preference" | "demonstration" | "instruction" => Some("high"),
            "run-summary" | "reflection" => Some("low"),
            _ => None,
        };
        // Keyword set: the key first (generic step name), then salient terms
        // derived from the content so keyword recall can match content queries.
        let mut keywords = vec![key.to_string()];
        keywords.extend(Self::content_keywords(content));
        let keywords = keywords.join(",");

        // ICM is best-effort: a failure must never break a run.
        let mut cmd = Command::new("icm");
        cmd.arg("store")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--content")
            .arg(content)
            .arg("--keywords")
            .arg(&keywords)
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--no-embeddings");
        if let Some(level) = importance {
            cmd.arg("--importance").arg(level);
        }
        let icm_id = match cmd.output().await {
            Ok(o) if o.status.success() => parse_icm_stored_id(&String::from_utf8_lossy(&o.stdout)),
            Ok(o) => {
                tracing::warn!(agent_id, stderr = %String::from_utf8_lossy(&o.stderr), "icm store failed (db still persisted)");
                None
            }
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "icm store failed (db still persisted)");
                None
            }
        };

        // For a consumer fork the data subject is that consumer unless the
        // caller says otherwise.
        let subject = prov
            .subject
            .clone()
            .or_else(|| scope.consumer_account().map(str::to_string));
        let source = prov
            .source
            .clone()
            .unwrap_or_else(|| source_for_key(key).to_string());
        sqlx::query(
            r#"INSERT INTO memories
                 (id, agent_id, consumer_account, key, content,
                  source, subject, legal_basis, retain_until, job_id, icm_id)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
        .bind(scope.consumer_account())
        .bind(key)
        .bind(content)
        .bind(&source)
        .bind(&subject)
        .bind(&prov.legal_basis)
        .bind(&prov.retain_until)
        .bind(&prov.job_id)
        .bind(&icm_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Erase every memory of `agent_id` whose data subject is `subject`, in any
    /// scope, on both sides (ICM by id where known, then the mirror). Returns
    /// the number of rows erased. This is the targeted right-to-erasure path;
    /// `forget` (whole scope) is the blunt one.
    pub async fn forget_subject(&self, agent_id: &str, subject: &str) -> Result<Erased> {
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, icm_id FROM memories WHERE agent_id = ? AND subject = ?")
                .bind(agent_id)
                .bind(subject)
                .fetch_all(&self.db)
                .await?;
        let mut erased = self.erase_rows(&rows).await?;
        // The subject is usually a consumer account: their whole fork topic goes
        // too (exact match), which also covers rows stored before ICM ids were
        // recorded. Owner-scope rows without an id stay reported as failures.
        let fork = MemoryScope::consumer(agent_id, subject);
        let has_fork: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM memories WHERE agent_id = ? AND consumer_account = ?",
        )
        .bind(agent_id)
        .bind(subject)
        .fetch_optional(&self.db)
        .await?;
        let fork_rows_left = has_fork.map(|r| r.0).unwrap_or(0);
        let fork_forget = Command::new("icm")
            .arg("forget")
            .arg("--topic")
            .arg(fork.topic())
            .arg("--db")
            .arg(&self.icm_db_path)
            .output()
            .await;
        if matches!(fork_forget, Ok(ref o) if o.status.success()) && fork_rows_left == 0 {
            // Every ICM copy of the fork is gone whatever the per-row outcome.
            erased.icm_failed = erased
                .icm_failed
                .saturating_sub(rows.iter().filter(|(_, icm)| icm.is_none()).count() as u64);
        }
        Ok(erased)
    }

    /// Erase one memory row (and its ICM entry) by mirror id, if it belongs to
    /// `agent_id`. Returns whether a row was erased.
    pub async fn forget_one(&self, agent_id: &str, memory_id: &str) -> Result<Option<Erased>> {
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, icm_id FROM memories WHERE agent_id = ? AND id = ?")
                .bind(agent_id)
                .bind(memory_id)
                .fetch_all(&self.db)
                .await?;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.erase_rows(&rows).await?))
    }

    /// Erase rows whose retention period has ended. Run by the maintenance loop.
    pub async fn expire_retained(&self) -> Result<Erased> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, icm_id FROM memories
             WHERE retain_until IS NOT NULL AND retain_until <= strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        )
        .fetch_all(&self.db)
        .await?;
        self.erase_rows(&rows).await
    }

    /// ICM forgets run concurrently (bounded), then the mirror rows go in one
    /// statement. Every ICM failure — unknown id or a failed `icm forget` — is
    /// counted and logged: recall consults ICM before the mirror, so a copy
    /// left there would still be served.
    async fn erase_rows(&self, rows: &[(String, Option<String>)]) -> Result<Erased> {
        use futures::stream::{self, StreamExt};
        if rows.is_empty() {
            return Ok(Erased::default());
        }
        let db_path = self.icm_db_path.clone();
        let icm_failed = stream::iter(rows.iter().cloned())
            .map(|(row_id, icm_id)| {
                let db_path = db_path.clone();
                async move {
                    let Some(icm_id) = icm_id else {
                        tracing::warn!(row_id, "memory has no ICM id; ICM copy (if any) not erased");
                        return 1u64;
                    };
                    match Command::new("icm")
                        .arg("forget")
                        .arg(&icm_id)
                        .arg("--db")
                        .arg(&db_path)
                        .output()
                        .await
                    {
                        Ok(o) if o.status.success() => 0,
                        Ok(o) => {
                            tracing::warn!(row_id, icm_id, stderr = %String::from_utf8_lossy(&o.stderr), "icm forget failed");
                            1
                        }
                        Err(e) => {
                            tracing::warn!(row_id, icm_id, error = %e, "icm forget could not run");
                            1
                        }
                    }
                }
            })
            .buffer_unordered(8)
            .fold(0u64, |acc, n| async move { acc + n })
            .await;

        let placeholders = vec!["?"; rows.len()].join(",");
        let sql = format!("DELETE FROM memories WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for (id, _) in rows {
            q = q.bind(id);
        }
        let removed = q.execute(&self.db).await?.rows_affected();
        Ok(Erased {
            rows: removed,
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
        let icm = Command::new("icm")
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
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--no-embeddings")
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
        self.store_with(scope, "correction", &lesson, &prov).await
    }

    /// Recall past corrections relevant to `query` (ICM feedback search).
    pub async fn recall_feedback(&self, scope: &MemoryScope, query: &str, limit: usize) -> String {
        let output = Command::new("icm")
            .arg("feedback")
            .arg("search")
            .arg(query)
            .arg("--topic")
            .arg(scope.topic())
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--no-embeddings")
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
        let out = Command::new("icm")
            .arg("stats")
            .arg("--db")
            .arg(&self.icm_db_path)
            .output()
            .await;
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
        let out = Command::new("icm")
            .arg("topics")
            .arg("--db")
            .arg(&self.icm_db_path)
            .output()
            .await;
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

    /// Purge one scope: its ICM topic and its rows in the DB mirror. An owner
    /// purge leaves every consumer fork in place, and vice versa.
    pub async fn forget(&self, scope: &MemoryScope) -> Result<()> {
        let _ = Command::new("icm")
            .arg("forget")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--db")
            .arg(&self.icm_db_path)
            .output()
            .await;
        sqlx::query("DELETE FROM memories WHERE agent_id = ? AND consumer_account IS ?")
            .bind(scope.agent_id())
            .bind(scope.consumer_account())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// All of an agent's stored ICM memories with their native importance
    /// metadata (weight, access_count, importance), used to rebuild the DB
    /// mirror after consolidation and to size/color the memory map.
    ///
    /// Enumerates the topic with `icm list`, NOT `icm recall <keyword>`:
    /// keyword recall (with `--no-embeddings`) only returns entries whose
    /// content matches the query term, so a generic query matched none of the
    /// agents' domain content and returned `[]` — which made the mirror resync
    /// log "parsed zero real entries" and never refresh. `list` returns the
    /// whole topic regardless of keywords.
    pub async fn icm_entries(&self, scope: &MemoryScope, limit: usize) -> Vec<IcmEntry> {
        let out = Command::new("icm")
            .arg("list")
            .arg("--topic")
            .arg(scope.topic())
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--db")
            .arg(&self.icm_db_path)
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

    /// Agent ids whose OWNER memory holds at least `min` entries.
    pub async fn agents_with_memory(&self, min: i64) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT agent_id FROM memories WHERE consumer_account IS NULL
             GROUP BY agent_id HAVING COUNT(*) >= ?",
        )
        .bind(min)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }

    /// Every scope (owner or consumer fork) holding at least `min_owner` /
    /// `min_consumer` verbatim entries — the candidates for consolidation.
    /// Consumer forks get a higher bar: each consolidation is an LLM call on the
    /// publisher's plan, and a popular agent has many forks.
    pub async fn scopes_to_consolidate(
        &self,
        min_owner: i64,
        min_consumer: i64,
    ) -> Vec<MemoryScope> {
        let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
            "SELECT agent_id, consumer_account, COUNT(*) FROM memories
             GROUP BY agent_id, consumer_account",
        )
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|(agent, consumer, n)| match consumer {
                None if n >= min_owner => Some(MemoryScope::owner(&agent)),
                Some(acc) if n >= min_consumer => Some(MemoryScope::consumer(&agent, &acc)),
                _ => None,
            })
            .collect()
    }

    /// Consolidate an agent's verbatim memories into a single distilled summary
    /// (ICM native consolidation, LLM summarizer via the inherited Max token).
    /// Best-effort: a failure must never break a run.
    pub async fn consolidate(&self, scope: &MemoryScope) {
        let topic = scope.topic();
        let out = Command::new("icm")
            .arg("consolidate")
            .arg("--topic")
            .arg(&topic)
            .arg("--summarizer-provider")
            .arg("claude")
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--no-embeddings")
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => {
                // Keep the DB mirror in sync: replace verbatim rows with the
                // consolidated summary so the UI and DB-fallback recall match.
                self.resync_mirror_from_icm(scope).await;
                tracing::info!(%topic, "consolidated agent memory");
            }
            Ok(o) => {
                tracing::warn!(%topic, stderr = %String::from_utf8_lossy(&o.stderr), "icm consolidate failed")
            }
            Err(e) => tracing::warn!(%topic, error = %e, "icm consolidate spawn failed"),
        }
    }

    /// Apply temporal decay to memory weights, then prune the faded ones.
    pub async fn decay_and_prune(&self) {
        let _ = Command::new("icm")
            .args(["decay", "--db", &self.icm_db_path, "--no-embeddings"])
            .output()
            .await;
        let _ = Command::new("icm")
            .args([
                "prune",
                "--threshold",
                "0.1",
                "--db",
                &self.icm_db_path,
                "--no-embeddings",
            ])
            .output()
            .await;
    }

    /// After ICM consolidation removed the originals, rebuild the DB mirror for
    /// this agent from what ICM now holds (the consolidated summary/entries).
    ///
    /// Safety: this MUST never wipe a healthy mirror. We first collect real,
    /// structured entries from ICM (via `icm_entries`, which parses JSON into
    /// `summary` fields — never a raw TOON header). Only if we obtain at least
    /// one non-empty entry do we replace the mirror, and we do so inside a single
    /// transaction so a mid-way failure can never leave the mirror empty.
    async fn resync_mirror_from_icm(&self, scope: &MemoryScope) {
        let agent_id = scope.agent_id();
        // Collect first; an empty Vec means "nothing real".
        let entries: Vec<IcmEntry> = self
            .icm_entries(scope, 50)
            .await
            .into_iter()
            // Drop any entry without a genuine summary so a header/placeholder
            // line can never be mirrored as a memory.
            .filter(|e| !e.summary.trim().is_empty())
            .collect();

        if entries.is_empty() {
            // Nothing real came back from ICM: leave the existing mirror intact
            // rather than destroying healthy memories.
            tracing::warn!(
                agent_id,
                "icm resync parsed zero real entries; keeping existing db mirror"
            );
            return;
        }

        // Replace the mirror atomically: delete + insert in one transaction so a
        // parse/insert failure can never leave the agent with an empty mirror.
        let mut tx = match self.db.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::warn!(agent_id, error = %e, "icm resync could not begin transaction; mirror untouched");
                return;
            }
        };

        // Scoped delete: consolidating the owner's memory must never wipe the
        // consumer forks (and vice versa). `IS ?` matches NULL for the owner.
        let retentions: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT retain_until FROM memories WHERE agent_id = ? AND consumer_account IS ?",
        )
        .bind(agent_id)
        .bind(scope.consumer_account())
        .fetch_all(&mut *tx)
        .await
        .unwrap_or_default();
        if let Err(e) =
            sqlx::query("DELETE FROM memories WHERE agent_id = ? AND consumer_account IS ?")
                .bind(agent_id)
                .bind(scope.consumer_account())
                .execute(&mut *tx)
                .await
        {
            tracing::warn!(agent_id, error = %e, "icm resync delete failed; rolling back, mirror untouched");
            return; // dropping `tx` rolls back automatically
        }

        // Provenance survives consolidation: the scope's subject and basis, and
        // the EARLIEST retention deadline of the rows being replaced (a
        // distilled memory may never outlive its strictest source).
        let (subject, basis, retain_until) = consolidated_provenance(scope, &retentions);
        for entry in &entries {
            if let Err(e) = sqlx::query(
                r#"INSERT INTO memories
                     (id, agent_id, consumer_account, key, content, source,
                      subject, legal_basis, retain_until, icm_id)
                   VALUES (?, ?, ?, 'consolidated', ?, 'consolidated', ?, ?, ?, ?)"#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(agent_id)
            .bind(scope.consumer_account())
            .bind(entry.summary.trim())
            .bind(&subject)
            .bind(&basis)
            .bind(&retain_until)
            .bind(&entry.id)
            .execute(&mut *tx)
            .await
            {
                tracing::warn!(agent_id, error = %e, "icm resync insert failed; rolling back, mirror untouched");
                return; // dropping `tx` rolls back automatically
            }
        }

        if let Err(e) = tx.commit().await {
            tracing::warn!(agent_id, error = %e, "icm resync commit failed; mirror untouched");
        }
    }

    /// List stored memories of one scope (UI / consumer API).
    pub async fn list(&self, scope: &MemoryScope) -> Result<Vec<MemoryEntry>> {
        let rows = sqlx::query_as::<_, MemoryEntry>(
            r#"SELECT id, agent_id, key, content, created_at,
                      source, subject, legal_basis, retain_until, job_id
               FROM memories WHERE agent_id = ? AND consumer_account IS ?
               ORDER BY created_at DESC"#,
        )
        .bind(scope.agent_id())
        .bind(scope.consumer_account())
        .fetch_all(&self.db)
        .await?;
        Ok(rows)
    }
}

/// Verbatim entries before an owner's memory is consolidated.
const OWNER_CONSOLIDATE_MIN: i64 = 6;
/// Verbatim entries before a consumer fork is consolidated. Higher on purpose:
/// every consolidation is an LLM call on the publisher's plan and a popular
/// agent has one fork per paying account.
const CONSUMER_CONSOLIDATE_MIN: i64 = 20;

/// Subject, legal basis and retention for the consolidated rows of a scope:
/// the scope's own subject/basis, and the earliest deadline among `retentions`.
fn consolidated_provenance(
    scope: &MemoryScope,
    retentions: &[Option<String>],
) -> (Option<String>, Option<String>, Option<String>) {
    let (subject, basis) = match scope {
        MemoryScope::Consumer { account_id, .. } => {
            (Some(account_id.clone()), Some("contract".to_string()))
        }
        MemoryScope::Owner { .. } => (None, None),
    };
    // Normalised deadlines sort chronologically as strings.
    let earliest = retentions.iter().flatten().min().cloned();
    (subject, basis, earliest)
}

/// Turn `icm recall --format json` output into prompt text, keeping only rows
/// whose `topic` is exactly `wanted` (most important first, as ICM orders
/// them). `None` when no row of that topic came back or the JSON is not a list.
fn render_recall(json: &str, wanted: &str) -> Option<String> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json.trim()).ok()?;
    let lines: Vec<String> = rows
        .iter()
        .filter(|r| r.get("topic").and_then(|t| t.as_str()) == Some(wanted))
        .filter_map(|r| {
            let summary = r.get("summary").and_then(|s| s.as_str())?.trim();
            if summary.is_empty() {
                return None;
            }
            let importance = r
                .get("importance")
                .and_then(|i| i.as_str())
                .unwrap_or("medium");
            Some(format!("- [{importance}] {summary}"))
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Join the owner's recalled memory and a consumer's own fork for the prompt.
fn compose_recall(owner: &str, own: &str) -> String {
    match (owner.trim().is_empty(), own.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => owner.to_string(),
        (true, false) => format!("What you have learnt about this user:\n{own}"),
        (false, false) => {
            format!("{owner}\n\nWhat you have learnt about this user (their own history with you):\n{own}")
        }
    }
}

/// Spawn the recurring background memory-maintenance loop: for each agent with
/// enough verbatim memories, consolidate them into a distilled summary (ICM
/// native), then apply temporal decay and prune faded entries. Keeps memory
/// from growing as a verbatim pile and surfaces the important/recent facts.
pub fn spawn_maintenance(memory: Memory, interval_secs: u64) {
    use std::time::Duration;
    let interval = Duration::from_secs(interval_secs);
    // Let the server settle before the first pass, but never wait longer than
    // one full interval (so a short demo cadence starts consolidating quickly).
    let settle = Duration::from_secs(interval_secs.min(120));
    tracing::info!(interval_secs, "memory maintenance loop started");
    tokio::spawn(async move {
        tokio::time::sleep(settle).await;
        loop {
            for scope in memory
                .scopes_to_consolidate(OWNER_CONSOLIDATE_MIN, CONSUMER_CONSOLIDATE_MIN)
                .await
            {
                memory.consolidate(&scope).await;
            }
            memory.decay_and_prune().await;
            match memory.expire_retained().await {
                Ok(e) if e.rows > 0 => tracing::info!(
                    erased = e.rows,
                    icm_failed = e.icm_failed,
                    "erased memories past their retention"
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "retention sweep failed"),
            }
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

    #[test]
    fn recall_rendering_keeps_only_the_exact_topic() {
        let json = r#"[
          {"topic":"takoia/agent/a1","summary":"owner fact","importance":"high"},
          {"topic":"takoia/agent/a10","summary":"sibling fact","importance":"medium"},
          {"topic":"takoia/fork/a1/c1","summary":"consumer fact","importance":"low"},
          {"topic":"takoia/agent/a1","summary":"   ","importance":"low"}
        ]"#;
        assert_eq!(
            render_recall(json, "takoia/agent/a1").as_deref(),
            Some("- [high] owner fact")
        );
        assert_eq!(
            render_recall(json, "takoia/fork/a1/c1").as_deref(),
            Some("- [low] consumer fact")
        );
        assert!(render_recall(json, "takoia/agent/a2").is_none());
        assert!(render_recall("memories[0]{id}:", "takoia/agent/a1").is_none());
        assert!(render_recall("[]", "takoia/agent/a1").is_none());
    }

    #[test]
    fn icm_stored_id_is_parsed_from_the_cli_line() {
        assert_eq!(
            parse_icm_stored_id("Stored: 01M3G4T5CM634F1X1RYBEGPMGJ (+5 links)\n").as_deref(),
            Some("01M3G4T5CM634F1X1RYBEGPMGJ")
        );
        assert_eq!(parse_icm_stored_id("Stored: \n"), None);
        assert_eq!(parse_icm_stored_id("No memories found."), None);
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
    fn consolidation_keeps_the_scope_subject_and_the_earliest_deadline() {
        let fork = MemoryScope::consumer("a", "acct");
        let (subject, basis, until) = consolidated_provenance(
            &fork,
            &[
                None,
                Some("2027-01-01T00:00:00.000Z".into()),
                Some("2026-06-01T00:00:00.000Z".into()),
            ],
        );
        assert_eq!(subject.as_deref(), Some("acct"));
        assert_eq!(basis.as_deref(), Some("contract"));
        assert_eq!(until.as_deref(), Some("2026-06-01T00:00:00.000Z"));
        let (subject, basis, until) = consolidated_provenance(&MemoryScope::owner("a"), &[None]);
        assert!(subject.is_none() && basis.is_none() && until.is_none());
    }

    #[test]
    fn source_defaults_follow_the_key() {
        assert_eq!(source_for_key("run-summary"), "run");
        assert_eq!(source_for_key("interaction"), "interaction");
        assert_eq!(source_for_key("analyse"), "step");
        assert_eq!(source_for_key("preference"), "manual");
        assert_eq!(source_for_key("whatever"), "run");
    }

    #[test]
    fn composed_recall_puts_the_personal_part_last() {
        assert_eq!(compose_recall("", ""), "");
        assert_eq!(compose_recall("expertise", ""), "expertise");
        let both = compose_recall("expertise", "likes brevity");
        assert!(both.starts_with("expertise"));
        assert!(both.ends_with("likes brevity"));
        assert!(compose_recall("", "likes brevity").contains("likes brevity"));
    }
}
