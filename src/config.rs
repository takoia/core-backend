//! Typed application configuration loaded from the environment.

use anyhow::{Context, Result};

/// Seed values for an LLM provider, read from the environment on first boot.
#[derive(Debug, Clone)]
pub struct ProviderSeed {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

/// Application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub frontend_dev_origin: String,
    pub database_url: String,
    /// 32-byte master key for credential encryption.
    pub master_key: [u8; 32],
    pub default_llm_provider: String,
    pub provider_seeds: Vec<ProviderSeed>,
    pub discord_webhook_url: Option<String>,
    /// Path to the dedicated ICM SQLite database for agent memory.
    pub icm_db_path: String,
    /// Optional Claude plan token (`claude setup-token`) used to seed claude_max.
    pub claude_max_token: Option<String>,
}

impl Config {
    /// Load configuration from the process environment (after `.env` is loaded).
    pub fn from_env() -> Result<Self> {
        let bind_addr = env_or("BIND_ADDR", "127.0.0.1:8080");
        let frontend_dev_origin = env_or("FRONTEND_DEV_ORIGIN", "http://localhost:5173");
        let database_url = env_or("DATABASE_URL", "sqlite://data/takoia.db?mode=rwc");

        let master_key = load_master_key()?;

        let default_llm_provider = env_or("DEFAULT_LLM_PROVIDER", "claude_max");
        let provider_seeds = load_provider_seeds();

        let discord_webhook_url = non_empty(std::env::var("DISCORD_WEBHOOK_URL").ok());
        let icm_db_path = env_or("ICM_DB_PATH", "data/icm.db");
        let claude_max_token = non_empty(std::env::var("CLAUDE_MAX_TOKEN").ok());

        Ok(Self {
            bind_addr,
            frontend_dev_origin,
            database_url,
            master_key,
            default_llm_provider,
            provider_seeds,
            discord_webhook_url,
            icm_db_path,
            claude_max_token,
        })
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

fn load_master_key() -> Result<[u8; 32]> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let raw = std::env::var("MASTER_KEY")
        .context("MASTER_KEY is required (base64-encoded 32 bytes; `openssl rand -base64 32`)")?;
    let bytes = STANDARD
        .decode(raw.trim())
        .context("MASTER_KEY must be valid base64")?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .context("MASTER_KEY must decode to exactly 32 bytes")?;
    Ok(arr)
}

/// Build provider seeds from the well-known `<NAME>_BASE_URL/_API_KEY/_MODEL` vars.
fn load_provider_seeds() -> Vec<ProviderSeed> {
    let specs = [
        ("claude_max", "CLAUDE_MAX"),
        ("ollama", "OLLAMA"),
        ("gemini", "GEMINI"),
        ("codex", "CODEX"),
    ];

    specs
        .iter()
        .filter_map(|(name, prefix)| {
            let base_url = non_empty(std::env::var(format!("{prefix}_BASE_URL")).ok())?;
            let api_key = non_empty(std::env::var(format!("{prefix}_API_KEY")).ok());
            let model = env_or(&format!("{prefix}_MODEL"), "");
            Some(ProviderSeed {
                name: name.to_string(),
                base_url,
                api_key,
                model,
            })
        })
        .collect()
}
