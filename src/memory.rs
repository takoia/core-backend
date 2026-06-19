//! Permanent agent memory — the "personalize your AI over time" capability.
//!
//! Backed by ICM (`icm` CLI) for semantic recall and consolidation, with the
//! `memories` table as the always-available source of truth for the UI and as a
//! fallback if ICM is unavailable. Each agent owns an ICM topic, so an expert
//! agent (e.g. trading) accumulates and refines expertise across runs.

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

/// Memory store bridging ICM and the local `memories` table.
#[derive(Clone)]
pub struct Memory {
    db: Db,
    icm_db_path: String,
}

impl Memory {
    pub fn new(db: Db, icm_db_path: String) -> Self {
        Self { db, icm_db_path }
    }

    fn topic(agent_id: &str) -> String {
        format!("takoia/agent/{agent_id}")
    }

    /// Recall expertise relevant to `query` for prompt injection at the Analyse
    /// step. Tries ICM first (semantic), falls back to recent DB memories.
    pub async fn recall(&self, agent_id: &str, query: &str, limit: usize) -> String {
        // 1) Query-scoped keyword recall.
        if let Some(text) = self.recall_icm(agent_id, query, limit).await {
            if Self::toon_has_entries(&text) {
                return text;
            }
        }
        // 2) Keyword recall frequently misses (memories carry generic step-name
        //    keywords, not content terms), in which case ICM returns an empty
        //    `memories[0]{...}` header. Fall back to the agent's highest-weight
        //    memories so accumulated expertise is ALWAYS injected.
        if let Some(text) = self.recall_top(agent_id).await {
            if !text.trim().is_empty() {
                return text;
            }
        }
        // 3) Last resort: the DB mirror.
        self.recall_db(agent_id, limit).await.unwrap_or_default()
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
    async fn recall_top(&self, agent_id: &str) -> Option<String> {
        let output = Command::new("icm")
            .arg("list")
            .arg("--topic")
            .arg(Self::topic(agent_id))
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

    async fn recall_icm(&self, agent_id: &str, query: &str, limit: usize) -> Option<String> {
        let output = Command::new("icm")
            .arg("recall")
            .arg(query)
            .arg("--topic")
            .arg(Self::topic(agent_id))
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
            tracing::warn!(agent_id, "icm recall failed, falling back to db memory");
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn recall_db(&self, agent_id: &str, limit: usize) -> Result<String> {
        let rows = sqlx::query_as::<_, MemoryEntry>(
            r#"SELECT id, agent_id, key, content, created_at
               FROM memories WHERE agent_id = ? ORDER BY created_at DESC LIMIT ?"#,
        )
        .bind(agent_id)
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
            "this", "that", "with", "from", "have", "they", "them", "then",
            "their", "there", "would", "could", "should", "about", "which",
            "when", "what", "were", "will", "your",
            "pour", "dans", "avec", "les", "des", "une", "que", "qui", "est",
            "sont", "cette", "vous", "nous", "mais", "comme", "plus",
        ];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for token in content
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
        {
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
    pub async fn store(&self, agent_id: &str, key: &str, content: &str) -> Result<()> {
        // User-specific memories are protected from decay/consolidation by
        // storing them at high importance; other (generic step) memories keep
        // ICM's default (medium).
        let high_importance = matches!(
            key,
            "correction" | "preference" | "demonstration" | "instruction"
        );
        // Keyword set: the key first (generic step name), then salient terms
        // derived from the content so keyword recall can match content queries.
        let mut keywords = vec![key.to_string()];
        keywords.extend(Self::content_keywords(content));
        let keywords = keywords.join(",");

        // ICM is best-effort: a failure must never break a run.
        let mut cmd = Command::new("icm");
        cmd.arg("store")
            .arg("--topic")
            .arg(Self::topic(agent_id))
            .arg("--content")
            .arg(content)
            .arg("--keywords")
            .arg(&keywords)
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--no-embeddings");
        if high_importance {
            cmd.arg("--importance").arg("high");
        }
        let icm = cmd.output().await;
        if let Err(e) = &icm {
            tracing::warn!(agent_id, error = %e, "icm store failed (db still persisted)");
        }

        sqlx::query(
            r#"INSERT INTO memories (id, agent_id, key, content) VALUES (?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
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
        agent_id: &str,
        context: &str,
        predicted: &str,
        corrected: &str,
        reason: &str,
    ) -> Result<()> {
        let icm = Command::new("icm")
            .arg("feedback")
            .arg("record")
            .arg("--topic")
            .arg(Self::topic(agent_id))
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
            tracing::warn!(agent_id, error = %e, "icm feedback record failed");
        }

        // Mirror as a high-signal memory so it is recalled at the Analyse step.
        let lesson = format!(
            "CORRECTION — when: {context}. Wrong: {predicted}. Correct: {corrected}. Reason: {reason}"
        );
        self.store(agent_id, "correction", &lesson).await
    }

    /// Recall past corrections relevant to `query` (ICM feedback search).
    pub async fn recall_feedback(&self, agent_id: &str, query: &str, limit: usize) -> String {
        let output = Command::new("icm")
            .arg("feedback")
            .arg("search")
            .arg(query)
            .arg("--topic")
            .arg(Self::topic(agent_id))
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
                let topic = l.rsplitn(2, char::is_whitespace).nth(1)?.trim().to_string();
                Some(serde_json::json!({ "topic": topic, "count": count }))
            })
            .collect()
    }

    /// Purge all memories in a topic (ICM forget --topic) and the DB mirror.
    pub async fn forget_topic(&self, topic: &str) -> Result<()> {
        let _ = Command::new("icm")
            .arg("forget")
            .arg("--topic")
            .arg(topic)
            .arg("--db")
            .arg(&self.icm_db_path)
            .output()
            .await;
        // Mirror: if it's an agent topic, clear the DB memories too.
        if let Some(agent_id) = topic.strip_prefix("takoia/agent/") {
            sqlx::query("DELETE FROM memories WHERE agent_id = ?")
                .bind(agent_id)
                .execute(&self.db)
                .await?;
        }
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
    pub async fn icm_entries(&self, agent_id: &str, _query: &str, limit: usize) -> Vec<IcmEntry> {
        let out = Command::new("icm")
            .arg("list")
            .arg("--topic")
            .arg(Self::topic(agent_id))
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
                        summary: m.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        weight: m.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0),
                        access_count: m.get("access_count").and_then(|v| v.as_i64()).unwrap_or(0),
                        importance: m.get("importance").and_then(|v| v.as_str()).unwrap_or("medium").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Agent ids that currently have at least `min` stored memories.
    pub async fn agents_with_memory(&self, min: i64) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT agent_id FROM memories GROUP BY agent_id HAVING COUNT(*) >= ?",
        )
        .bind(min)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
    }

    /// Consolidate an agent's verbatim memories into a single distilled summary
    /// (ICM native consolidation, LLM summarizer via the inherited Max token).
    /// Best-effort: a failure must never break a run.
    pub async fn consolidate(&self, agent_id: &str) {
        let out = Command::new("icm")
            .arg("consolidate")
            .arg("--topic")
            .arg(Self::topic(agent_id))
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
                self.resync_mirror_from_icm(agent_id).await;
                tracing::info!(agent_id, "consolidated agent memory");
            }
            Ok(o) => tracing::warn!(agent_id, stderr = %String::from_utf8_lossy(&o.stderr), "icm consolidate failed"),
            Err(e) => tracing::warn!(agent_id, error = %e, "icm consolidate spawn failed"),
        }
    }

    /// Apply temporal decay to memory weights, then prune the faded ones.
    pub async fn decay_and_prune(&self) {
        let _ = Command::new("icm")
            .args(["decay", "--db", &self.icm_db_path, "--no-embeddings"])
            .output()
            .await;
        let _ = Command::new("icm")
            .args(["prune", "--threshold", "0.1", "--db", &self.icm_db_path, "--no-embeddings"])
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
    async fn resync_mirror_from_icm(&self, agent_id: &str) {
        // Collect first. `icm_entries` substitutes a safe keyword for blank
        // queries and yields parsed entries; an empty Vec means "nothing real".
        let entries: Vec<IcmEntry> = self
            .icm_entries(agent_id, "memory", 50)
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

        if let Err(e) = sqlx::query("DELETE FROM memories WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await
        {
            tracing::warn!(agent_id, error = %e, "icm resync delete failed; rolling back, mirror untouched");
            return; // dropping `tx` rolls back automatically
        }

        for entry in &entries {
            if let Err(e) = sqlx::query(
                r#"INSERT INTO memories (id, agent_id, key, content) VALUES (?, ?, 'consolidated', ?)"#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(agent_id)
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

    /// List stored memories for an agent (UI).
    pub async fn list(&self, agent_id: &str) -> Result<Vec<MemoryEntry>> {
        let rows = sqlx::query_as::<_, MemoryEntry>(
            r#"SELECT id, agent_id, key, content, created_at
               FROM memories WHERE agent_id = ? ORDER BY created_at DESC"#,
        )
        .bind(agent_id)
        .fetch_all(&self.db)
        .await?;
        Ok(rows)
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
            for agent_id in memory.agents_with_memory(6).await {
                memory.consolidate(&agent_id).await;
            }
            memory.decay_and_prune().await;
            tokio::time::sleep(interval).await;
        }
    });
}
