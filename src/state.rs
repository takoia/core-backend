//! Shared application state passed to every HTTP handler and the worker.

use crate::config::Config;
use crate::crypto::Cipher;
use crate::db::Db;
use crate::memory::Memory;
use std::sync::Arc;

/// Cheaply-cloneable shared state (everything behind an `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub cipher: Cipher,
    pub memory: Memory,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let cipher = Cipher::new(config.master_key);
        let memory = Memory::new(db.clone(), config.icm_db_path.clone());
        Self {
            db,
            config: Arc::new(config),
            cipher,
            memory,
        }
    }
}
