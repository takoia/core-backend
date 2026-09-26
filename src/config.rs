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
    /// Path to the dedicated ICM SQLite database for agent memory.
    pub icm_db_path: String,
    /// Optional Claude plan token (`claude setup-token`) used to seed claude_max.
    pub claude_max_token: Option<String>,
    /// Neutral working directory for agent `claude -p` subprocesses, isolated
    /// from the host project (no CLAUDE.md / project ICM contamination).
    pub agent_workdir: String,
    /// Admin username for the login page.
    pub admin_username: String,
    /// Admin password, only set when `ADMIN_PASSWORD` is provided. When `None`,
    /// no admin is seeded and the first-run setup wizard creates it instead.
    pub admin_password: Option<String>,
    /// How often the background memory maintenance pass runs (consolidate +
    /// decay + prune), in seconds. Lower it to see memory grow in near real time.
    pub memory_maintenance_interval_secs: u64,
    /// How often the inner-life pass runs (reflection, mood update, initiative,
    /// kept commitments), in seconds. Lower it to watch the agent come alive.
    pub inner_life_interval_secs: u64,
    /// Demo mode (`DEMO_MODE=true`): when no LLM provider answers, steps fall
    /// back to the offline canned provider so a run still completes. Off by
    /// default: in production a provider failure fails the run instead of
    /// delivering demo text as if it were real.
    pub demo_mode: bool,
    /// Per-model notional pricing (`LLM_PRICING`, `LLM_PRICING_DEFAULT`).
    pub pricing: crate::pricing::Pricing,
    /// A synchronous job (marketplace invoke, call_agent sub-run) still
    /// `running` after this many seconds is considered abandoned and failed.
    /// Generous by default (`SYNC_JOB_MAX_SECS`, 4h): nested sub-runs and web
    /// searches legitimately take long, and a false positive fails a paying
    /// call after its tokens were spent.
    pub sync_job_max_secs: i64,
    /// Extra environment variable names passed through to agent subprocesses
    /// (`AGENT_ENV_PASSTHROUGH`, comma-separated), on top of the built-in
    /// proxy and CA variables.
    pub agent_env_passthrough: Vec<String>,
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

        let icm_db_path = env_or("ICM_DB_PATH", "data/icm.db");
        let claude_max_token = non_empty(std::env::var("CLAUDE_MAX_TOKEN").ok());
        // Absolute path OUTSIDE the project git tree so the agent's claude -p
        // does not walk up to the host CLAUDE.md or trigger project ICM recall.
        let agent_workdir = env_or("AGENT_WORKDIR", "/tmp/takoia-agent-workspace");

        let admin_username = env_or("ADMIN_USERNAME", "admin");
        let admin_password = std::env::var("ADMIN_PASSWORD")
            .ok()
            .filter(|p| !p.trim().is_empty());

        // Memory maintenance cadence. Default 300s (5 min) so consolidation is
        // visible without hammering the LLM; clamped to >= 30s.
        let memory_maintenance_interval_secs = std::env::var("MEMORY_MAINTENANCE_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|n| *n >= 30)
            .unwrap_or(300);

        // Inner-life cadence (reflection/mood/initiative). Default 900s (15 min);
        // clamped to >= 60s. Each pass is one LLM call per agent with memory.
        let inner_life_interval_secs = std::env::var("INNER_LIFE_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|n| *n >= 60)
            .unwrap_or(900);

        let demo_mode = std::env::var("DEMO_MODE")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        let pricing = crate::pricing::Pricing::from_env(
            std::env::var("LLM_PRICING").ok().as_deref(),
            std::env::var("LLM_PRICING_DEFAULT").ok().as_deref(),
        )?;

        let sync_job_max_secs = std::env::var("SYNC_JOB_MAX_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|n| *n >= 60)
            .unwrap_or(4 * 3600);
        let agent_env_passthrough: Vec<String> = std::env::var("AGENT_ENV_PASSTHROUGH")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            bind_addr,
            frontend_dev_origin,
            database_url,
            master_key,
            default_llm_provider,
            provider_seeds,
            icm_db_path,
            claude_max_token,
            agent_workdir,
            admin_username,
            admin_password,
            memory_maintenance_interval_secs,
            inner_life_interval_secs,
            demo_mode,
            pricing,
            sync_job_max_secs,
            agent_env_passthrough,
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
