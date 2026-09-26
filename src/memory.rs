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
    pub summary: String,
    pub weight: f64,
    pub access_count: i64,
    pub importance: String,
}

/// A stored memory entry, surfaced in the UI.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MemoryEntry {
    pub id: String,
    pub agent_id: String,
    pub key: String,
    pub content: String,
    pub created_at: String,
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
const CONSUMER_SEGMENT: &str = "/consumer/";

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
            } => format!("{TOPIC_PREFIX}{agent_id}{CONSUMER_SEGMENT}{account_id}"),
        }
    }

    /// Inverse of [`topic`](Self::topic). Rejects anything that is not exactly
    /// one of the two shapes (a bare `strip_prefix` would read a consumer topic
    /// as an owner topic with a bogus agent id).
    pub fn parse_topic(topic: &str) -> Option<MemoryScope> {
        let rest = topic.strip_prefix(TOPIC_PREFIX)?;
        if rest.is_empty() {
            return None;
        }
        match rest.split_once(CONSUMER_SEGMENT) {
            None if !rest.contains('/') => Some(MemoryScope::owner(rest)),
            Some((agent, account))
                if !agent.is_empty()
                    && !account.is_empty()
                    && !agent.contains('/')
                    && !account.contains('/') =>
            {
                Some(MemoryScope::consumer(agent, account))
            }
            _ => None,
        }
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
        let owner: String = self
            .recall(&MemoryScope::owner(agent_id), query, limit)
            .await
            .chars()
            .take(OWNER_RECALL_CHARS)
            .collect();
        let Some(account) = consumer_account else {
            return owner;
        };
        let own: String = self
            .recall(&MemoryScope::consumer(agent_id, account), query, limit)
            .await
            .chars()
            .take(CONSUMER_RECALL_CHARS)
            .collect();
        compose_recall(&owner, &own)
    }

    /// Recall expertise relevant to `query` for prompt injection at the Analyse
    /// step. Tries ICM first (semantic), falls back to recent DB memories.
    pub async fn recall(&self, scope: &MemoryScope, query: &str, limit: usize) -> String {
        // 1) Query-scoped keyword recall.
        if let Some(text) = self.recall_icm(scope, query, limit).await {
            if Self::toon_has_entries(&text) {
                return text;
            }
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

    async fn recall_icm(&self, scope: &MemoryScope, query: &str, limit: usize) -> Option<String> {
        let output = Command::new("icm")
            .arg("recall")
            .arg(query)
            .arg("--topic")
            .arg(scope.topic())
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--limit")
            .arg(limit.to_string())
            .arg("--format")
            .arg("toon")
            // Keyword search, matching how we store (no embedding model download).
            .arg("--no-embeddings")
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            tracing::warn!(topic = %scope.topic(), "icm recall failed, falling back to db memory");
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn recall_db(&self, scope: &MemoryScope, limit: usize) -> Result<String> {
        // `IS ?` so a NULL bind matches the owner rows (`= NULL` never matches).
        let rows = sqlx::query_as::<_, MemoryEntry>(
            r#"SELECT id, agent_id, key, content, created_at
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

    /// Persist a new memory at the Restitution step: ICM (best-effort) + DB.
    pub async fn store(&self, scope: &MemoryScope, key: &str, content: &str) -> Result<()> {
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
        let icm = cmd.output().await;
        if let Err(e) = &icm {
            tracing::warn!(agent_id, error = %e, "icm store failed (db still persisted)");
        }

        sqlx::query(
            r#"INSERT INTO memories (id, agent_id, consumer_account, key, content)
               VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
        .bind(scope.consumer_account())
        .bind(key)
        .bind(content)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Record a correction (what the agent predicted vs the correct answer) so
    /// the agent improves next time. Backed by ICM feedback, mirrored to DB.
    pub async fn record_feedback(
        &self,
        scope: &MemoryScope,
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
        self.store(scope, "correction", &lesson).await
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

        for entry in &entries {
            if let Err(e) = sqlx::query(
                r#"INSERT INTO memories (id, agent_id, consumer_account, key, content)
                   VALUES (?, ?, ?, 'consolidated', ?)"#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(agent_id)
            .bind(scope.consumer_account())
            .bind(entry.summary.trim())
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
            r#"SELECT id, agent_id, key, content, created_at
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
        assert_eq!(c.topic(), "takoia/agent/agent-1/consumer/acct-b");
        assert_eq!(MemoryScope::parse_topic(&c.topic()), Some(c.clone()));
        assert_eq!(c.consumer_account(), Some("acct-b"));
        assert_eq!(c.agent_id(), "agent-1");
    }

    #[test]
    fn parse_rejects_malformed_topics() {
        for t in [
            "takoia/agent/",
            "other/agent/x",
            "takoia/agent/a/consumer/",
            "takoia/agent//consumer/b",
            "takoia/agent/a/b",
            "takoia/agent/a/consumer/b/c",
            "",
        ] {
            assert!(
                MemoryScope::parse_topic(t).is_none(),
                "{t:?} must not parse"
            );
        }
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
