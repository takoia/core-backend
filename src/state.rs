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
    /// Agents whose episodes are being distilled right now, so the maintenance
    /// loop and a publication never distil the same agent at once.
    pub distilling: crate::distill::InFlight,
    /// Per-event attempt limiter for the public webhook route.
    pub webhook_limiter: Arc<crate::ratelimit::SlidingWindow>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let cipher = Cipher::new(config.master_key);
        let memory = Memory::new(db.clone(), config.icm_db_path.clone())
            .with_embeddings(config.memory_embeddings);
        let events = EventBus::new(db.clone());
        let webhook_limiter = Arc::new(crate::ratelimit::SlidingWindow::new(
            config.webhook_rate_limit_per_min,
        ));
        Self {
            db,
            config: Arc::new(config),
            cipher,
            memory,
            events,
            distilling: crate::distill::InFlight::default(),
            webhook_limiter,
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
        let sandbox = crate::sandbox::active(&self.db)
            .await
            .with_passthrough(&self.config.agent_env_passthrough);
        ProviderRegistry::load(
            &self.db,
            &self.cipher,
            account_id,
            &self.config.default_llm_provider,
            &workdir,
            &sandbox,
            self.config.demo_mode,
        )
        .await
    }
}
