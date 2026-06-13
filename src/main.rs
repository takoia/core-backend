//! TakoIA core-backend entrypoint: load config, open the DB, run migrations,
//! and serve the HTTP API plus the static frontend.

mod agent;
mod agentdef;
mod bootstrap;
mod config;
mod crypto;
mod db;
mod domain;
mod error;
mod http;
mod llm;
mod memory;
mod queue;
mod state;
mod tools;

use anyhow::{Context, Result};
use config::Config;
use state::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    // Load `.env` if present; real environment always wins.
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = Config::from_env().context("failed to load configuration")?;
    tracing::info!(bind = %config.bind_addr, db = %config.database_url, "starting takoia-core");

    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");

    let state = AppState::new(pool, config.clone());

    // First-boot seeding: default account, providers, demo agent.
    bootstrap::run(&state.db, &state.cipher, &state.config).await?;

    // Background job worker (runs the agent engine).
    agent::worker::spawn(state.clone());

    let app = http::router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "listening");

    axum::serve(listener, app)
        .await
        .context("server error")?;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("takoia=debug,tower_http=info,info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
