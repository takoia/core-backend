//! First-boot seeding: a default account, LLM provider connectors (from env),
//! and a pre-configured demo expert agent so the red-thread demo runs instantly.

use crate::config::Config;
use crate::crypto::Cipher;
use crate::db::Db;
use crate::domain::StepType;
use crate::llm::CLAUDE_CLI_SENTINEL;
use anyhow::Result;
use uuid::Uuid;

/// Fixed single-tenant account for the demo (no auth in the MVP).
pub const DEFAULT_ACCOUNT_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Ensure the default account, seed providers, and seed the demo agent.
pub async fn run(db: &Db, cipher: &Cipher, config: &Config) -> Result<()> {
    ensure_account(db).await?;
    seed_providers(db, cipher, config).await?;
    seed_demo_agent(db).await?;
    seed_invoice_agent(db).await?;
    seed_example_agents(db).await?;
    Ok(())
}

/// Seed a few ready-to-run example agents so the demo has content immediately.
async fn seed_example_agents(db: &Db) -> Result<()> {
    // id, name, expertise, autonomy, trigger_on, emit, action_tools
    let examples = [
        ("weather-watch", "Weather Watcher", "weather", "full_auto", "schedule.daily", "weather.ready", true),
        ("market-pulse", "Market Pulse", "market intelligence", "full_auto", "schedule.hourly", "market.ready", true),
        ("competitor-watch", "Competitor Watch", "competitive intelligence", "confirm_before_action", "", "competitor.report", true),
        ("social-monitor", "Social Monitor", "social media monitoring", "full_auto", "", "social.summary", true),
    ];
    for (id, name, expertise, autonomy, trigger_on, emit, web) in examples {
        let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM agents WHERE id = ?")
            .bind(id)
            .fetch_optional(db)
            .await?;
        if exists.is_some() {
            continue;
        }
        let trig = if trigger_on.is_empty() { None } else { Some(trigger_on) };
        sqlx::query(
            r#"INSERT INTO agents
               (id, account_id, name, description, autonomy_level, expertise_domain,
                author, visibility, trigger_on, emit)
               VALUES (?, ?, ?, ?, ?, ?, 'TakoIA examples', 'public', ?, ?)"#,
        )
        .bind(id)
        .bind(DEFAULT_ACCOUNT_ID)
        .bind(name)
        .bind(format!("Example {expertise} agent — researches and reports."))
        .bind(autonomy)
        .bind(expertise)
        .bind(trig)
        .bind(format!("[\"{emit}\"]"))
        .execute(db)
        .await?;
        for (pos, step) in StepType::ALL.iter().enumerate() {
            let options = if *step == StepType::Action && web {
                serde_json::json!({ "allowed_tools": ["web_search"] })
            } else {
                serde_json::json!({})
            };
            sqlx::query(
                r#"INSERT INTO agent_step_configs (id, agent_id, step_type, system_prompt, options, position)
                   VALUES (?, ?, ?, '', ?, ?)"#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(id)
            .bind(step.as_str())
            .bind(options.to_string())
            .bind(pos as i64)
            .execute(db)
            .await?;
        }
    }
    tracing::info!("seeded example agents");
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

async fn seed_demo_agent(db: &Db) -> Result<()> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agents WHERE account_id = ?")
        .bind(DEFAULT_ACCOUNT_ID)
        .fetch_one(db)
        .await?;
    if count.0 > 0 {
        return Ok(());
    }

    let agent_id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO agents
           (id, account_id, name, description, autonomy_level, expertise_domain, visibility)
           VALUES (?, ?, ?, ?, 'confirm_before_action', 'AI agents technology watch', 'public')"#,
    )
    .bind(&agent_id)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind("Tech Watch Expert")
    .bind("Monitors recent AI-agent announcements and produces a weekly synthesis.")
    .execute(db)
    .await?;

    // One config row per step; empty system_prompt means "use the default".
    for (pos, step) in StepType::ALL.iter().enumerate() {
        let options = if *step == StepType::Action {
            serde_json::json!({ "allowed_tools": ["web_search"] })
        } else {
            serde_json::json!({})
        };
        sqlx::query(
            r#"INSERT INTO agent_step_configs
               (id, agent_id, step_type, system_prompt, options, position)
               VALUES (?, ?, ?, '', ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&agent_id)
        .bind(step.as_str())
        .bind(options.to_string())
        .bind(pos as i64)
        .execute(db)
        .await?;
    }

    tracing::info!(agent_id = %agent_id, "seeded demo agent 'Tech Watch Expert'");
    Ok(())
}

/// Seed the invoice-extraction agent triggered by the `invoice.received`
/// webhook. Full-auto so inbound invoices are processed automatically; it learns
/// from corrections over time.
async fn seed_invoice_agent(db: &Db) -> Result<()> {
    let id = "invoice-extractor";
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM agents WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;
    if exists.is_some() {
        return Ok(());
    }

    sqlx::query(
        r#"INSERT INTO agents
           (id, account_id, name, description, autonomy_level, expertise_domain,
            author, visibility, trigger_on, emit)
           VALUES (?, ?, ?, ?, 'full_auto', 'invoice data extraction',
                   'TakoIA demo', 'public', 'invoice.received', '["invoice.extracted"]')"#,
    )
    .bind(id)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind("Invoice Extractor")
    .bind("Receives invoices via webhook and returns the extracted text fields. Improves from corrections.")
    .execute(db)
    .await?;

    let prompts = [
        (
            StepType::Analyse,
            "Identify which invoice fields are present in the payload: supplier, \
             invoice number, date, line items, totals, taxes, currency.",
            serde_json::json!({}),
        ),
        (
            StepType::Decision,
            "Decide how to map the payload to a clean structured extraction. \
             Apply any past corrections provided.",
            serde_json::json!({}),
        ),
        (
            StepType::Action,
            "Extract the fields from the input. Do not search the web.",
            serde_json::json!({ "allowed_tools": [] }),
        ),
        (
            StepType::Restitution,
            "Return the extracted invoice as clean text with one field per line: \
             Supplier, Invoice number, Date, Currency, Line items, Subtotal, Tax, \
             Total. Use 'unknown' for any missing field. No commentary.",
            serde_json::json!({}),
        ),
    ];
    for (pos, (step, prompt, options)) in prompts.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO agent_step_configs
               (id, agent_id, step_type, system_prompt, options, position)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(id)
        .bind(step.as_str())
        .bind(prompt)
        .bind(options.to_string())
        .bind(pos as i64)
        .execute(db)
        .await?;
    }

    tracing::info!("seeded invoice-extractor agent");
    Ok(())
}
