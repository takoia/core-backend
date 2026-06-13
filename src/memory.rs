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
        if let Some(text) = self.recall_icm(agent_id, query, limit).await {
            if !text.trim().is_empty() {
                return text;
            }
        }
        self.recall_db(agent_id, limit).await.unwrap_or_default()
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

    /// Persist a new memory at the Restitution step: ICM (best-effort) + DB.
    pub async fn store(&self, agent_id: &str, key: &str, content: &str) -> Result<()> {
        // ICM is best-effort: a failure must never break a run.
        let icm = Command::new("icm")
            .arg("store")
            .arg("--topic")
            .arg(Self::topic(agent_id))
            .arg("--content")
            .arg(content)
            .arg("--keywords")
            .arg(key)
            .arg("--db")
            .arg(&self.icm_db_path)
            .arg("--no-embeddings")
            .output()
            .await;
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
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
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

    /// Number of stored memories for an agent (DB mirror).
    pub async fn count(&self, agent_id: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM memories WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_one(&self.db)
            .await
            .unwrap_or(0)
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
    /// this agent from what ICM now holds (the consolidated summary).
    async fn resync_mirror_from_icm(&self, agent_id: &str) {
        let recalled = self.recall_icm(agent_id, "", 50).await.unwrap_or_default();
        if recalled.trim().is_empty() {
            return;
        }
        let _ = sqlx::query("DELETE FROM memories WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&self.db)
            .await;
        let _ = sqlx::query(
            r#"INSERT INTO memories (id, agent_id, key, content) VALUES (?, ?, 'consolidated', ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id)
        .bind(recalled.trim())
        .execute(&self.db)
        .await;
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
pub fn spawn_maintenance(memory: Memory) {
    use std::time::Duration;
    tokio::spawn(async move {
        // Let the server settle before the first pass.
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            for agent_id in memory.agents_with_memory(6).await {
                memory.consolidate(&agent_id).await;
            }
            memory.decay_and_prune().await;
            tokio::time::sleep(Duration::from_secs(1800)).await; // every 30 min
        }
    });
}
