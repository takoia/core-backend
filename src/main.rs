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
mod sandbox;
mod scheduler;
mod security;
mod secrets;
mod state;
mod tools;

use anyhow::{Context, Result};
use config::Config;
use state::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    // Throwaway sandbox self-test: `takoia sandbox-selftest [workdir]`. Runs on a
    // dedicated thread (Landlock restrict_self is per-thread) and exits.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("sandbox-selftest") {
        let workdir = args.get(2).cloned().unwrap_or_else(|| "/tmp/ll-selftest".to_string());
        let report = std::thread::spawn(move || sandbox::selftest(&workdir))
            .join()
            .unwrap_or_else(|_| "selftest thread panicked".to_string());
        print!("{report}");
        return Ok(());
    }

    // Load `.env` if present; real environment always wins.
    let _ = dotenvy::dotenv();
    init_tracing();

    let config = Config::from_env().context("failed to load configuration")?;
    tracing::info!(bind = %config.bind_addr, db = %config.database_url, "starting takoia-core");
    match &config.admin_password {
        Some(_) => tracing::info!(
            username = %config.admin_username,
            "admin seeded from ADMIN_PASSWORD"
        ),
        None => tracing::info!(
            "no ADMIN_PASSWORD set; first-run setup wizard will create the admin on first visit"
        ),
    }

    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");

    let state = AppState::new(pool, config.clone());

    // First-boot seeding: default account, providers, demo agent.
    bootstrap::run(&state.db, &state.cipher, &state.config).await?;

    // Seed the initial admin user from env credentials (multi-user auth).
    http::users::ensure_admin_user(&state).await?;

    // Background job worker (runs the agent engine).
    agent::worker::spawn(state.clone());

    // Recurring scheduler (autonomous learning loops).
    scheduler::spawn(state.clone());

    // Background memory maintenance: consolidate / decay / prune ICM memories.
    memory::spawn_maintenance(
        state.memory.clone(),
        config.memory_maintenance_interval_secs,
    );

    // Inner life: reflection, mood drift, initiative, and kept commitments.
    agent::inner_life::spawn(state.clone(), config.inner_life_interval_secs);

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
