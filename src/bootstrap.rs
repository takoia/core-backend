//! First-boot seeding: a default account, LLM provider connectors (from env),
//! and a pre-configured demo expert agent so the red-thread demo runs instantly.

use crate::config::Config;
use crate::crypto::Cipher;
use crate::db::Db;
use crate::llm::CLAUDE_CLI_SENTINEL;
use anyhow::Result;
use uuid::Uuid;

/// Fixed single-tenant account for the demo (no auth in the MVP).
pub const DEFAULT_ACCOUNT_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Ensure the default account, seed providers, and seed the demo agent.
pub async fn run(db: &Db, cipher: &Cipher, config: &Config) -> Result<()> {
    ensure_account(db).await?;
    seed_providers(db, cipher, config).await?;
    seed_showcase_agent(db).await?;
    Ok(())
}

/// On a FRESH install (no agents yet), seed a single showcase agent: a real
/// windsurf/kitesurf weather scout for Wissant that fetches live wind from the
/// free Open-Meteo API and alerts only on a rideable session. Demonstrates a
/// data-backed agent end to end. Disable with `SEED_SHOWCASE_AGENT=false`.
async fn seed_showcase_agent(db: &Db) -> Result<()> {
    let disabled = std::env::var("SEED_SHOWCASE_AGENT")
        .map(|v| matches!(v.trim(), "false" | "0" | "no"))
        .unwrap_or(false);
    if disabled {
        return Ok(());
    }
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agents")
        .fetch_one(db)
        .await?;
    if count.0 > 0 {
        return Ok(()); // not a fresh install — never recreate a deleted agent.
    }
    const SEED: &str = include_str!("seed/windsurf-wissant.toml");
    let id = crate::agentdef::import(db, DEFAULT_ACCOUNT_ID, SEED).await?;
    // Check the spot every 6 hours; NULL next_run_at runs it on the next tick.
    sqlx::query(
        r#"INSERT INTO schedules
             (id, agent_id, title, prompt, cron_expr, interval_seconds, enabled, next_run_at)
           VALUES (?, ?, ?, ?, '', 21600, 1, NULL)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&id)
    .bind("Windsurf session check — Wissant")
    .bind("Check today's windsurf/kitesurf conditions at Wissant; alert only on a rideable window.")
    .execute(db)
    .await?;
    tracing::info!(agent_id = %id, "seeded showcase windsurf-weather agent");
    Ok(())
}

async fn ensure_account(db: &Db) -> Result<()> {
    sqlx::query(
        "INSERT INTO accounts (id, name) VALUES (?, 'Demo') ON CONFLICT(id) DO NOTHING",
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .execute(db)
    .await?;
    Ok(())
}

async fn seed_providers(db: &Db, cipher: &Cipher, config: &Config) -> Result<()> {
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM connectors WHERE account_id = ? AND kind = 'llm'")
            .bind(DEFAULT_ACCOUNT_ID)
            .fetch_one(db)
            .await?;
    if count.0 > 0 {
        return Ok(()); // already configured (possibly from the Settings UI)
    }

    // claude_max via the Claude Code headless transport (consumes the plan).
    let claude_secret = config
        .claude_max_token
        .as_ref()
        .map(|t| cipher.encrypt(t))
        .transpose()?;
    insert_connector(
        db,
        "claude_max",
        CLAUDE_CLI_SENTINEL,
        "", // empty -> claude -p uses its default plan model
        claude_secret,
        true,
    )
    .await?;

    // OpenAI-compatible providers from env seeds (ollama / gemini / codex).
    for seed in &config.provider_seeds {
        if seed.name == "claude_max" {
            continue;
        }
        let secret = seed.api_key.as_ref().map(|k| cipher.encrypt(k)).transpose()?;
        insert_connector(db, &seed.name, &seed.base_url, &seed.model, secret, false).await?;
    }

    tracing::info!("seeded default LLM providers");
    Ok(())
}

async fn insert_connector(
    db: &Db,
    name: &str,
    base_url: &str,
    model: &str,
    secret: Option<Vec<u8>>,
    is_default: bool,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO connectors
           (id, account_id, kind, name, base_url, model, encrypted_secret, is_default)
           VALUES (?, ?, 'llm', ?, ?, ?, ?, ?)
           ON CONFLICT(account_id, kind, name) DO NOTHING"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(name)
    .bind(base_url)
    .bind(model)
    .bind(secret)
    .bind(is_default as i64)
    .execute(db)
    .await?;
    Ok(())
}
