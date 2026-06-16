//! Shared application state passed to every HTTP handler and the worker.

use crate::agent::EventBus;
use crate::config::Config;
use crate::crypto::Cipher;
use crate::db::Db;
use crate::llm::ProviderRegistry;
use crate::memory::Memory;
use anyhow::Result;
use std::sync::Arc;

/// Cheaply-cloneable shared state (everything behind an `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub cipher: Cipher,
    pub memory: Memory,
    pub events: EventBus,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let cipher = Cipher::new(config.master_key);
        let memory = Memory::new(db.clone(), config.icm_db_path.clone());
        let events = EventBus::new(db.clone());
        Self {
            db,
            config: Arc::new(config),
            cipher,
            memory,
            events,
        }
    }

    /// Build the LLM provider registry for an account (loads + decrypts
    /// connectors, with the global default as the fallback provider name).
    pub async fn load_registry(
        &self,
        account_id: &str,
        agent_id: &str,
    ) -> Result<ProviderRegistry> {
        // Per-agent workdir so agents are isolated from each other, and the
        // active execution sandbox confines the subprocess to it.
        let workdir = format!("{}/{}", self.config.agent_workdir, agent_id);
        let sandbox = crate::sandbox::active(&self.db).await;
        ProviderRegistry::load(
            &self.db,
            &self.cipher,
            account_id,
            &self.config.default_llm_provider,
            &workdir,
            &sandbox,
        )
        .await
    }
}
