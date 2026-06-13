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
