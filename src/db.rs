//! Database pool setup and migrations.
//!
//! SQLite by default (single binary, zero infra). The `DATABASE_URL` drives the
//! choice; switching to Postgres later means swapping the pool type here.

use anyhow::{Context, Result};
use sqlx::sqlite::{SqlitePoolOptions, SqliteConnectOptions, SqliteJournalMode};
use sqlx::SqlitePool;
use std::str::FromStr;

/// The shared connection pool type used across the app.
pub type Db = SqlitePool;

/// Create the pool, ensuring the SQLite file exists and WAL mode is enabled.
pub async fn connect(database_url: &str) -> Result<Db> {
    ensure_parent_dir(database_url);

    let connect_opts = SqliteConnectOptions::from_str(database_url)
        .context("invalid DATABASE_URL")?
        .create_if_missing(true)
        // WAL lets a reader run while the single writer works: needed for SSE
        // reads while the worker writes step progress.
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(connect_opts)
        .await
        .context("failed to connect to database")?;

    Ok(pool)
}

/// Apply all embedded migrations from the `migrations/` directory.
pub async fn migrate(pool: &Db) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("failed to run migrations")?;
    Ok(())
}

/// Create the parent directory of a `sqlite://path` URL if needed.
fn ensure_parent_dir(database_url: &str) {
    let path = database_url
        .strip_prefix("sqlite://")
        .map(|rest| rest.split('?').next().unwrap_or(rest));
    if let Some(path) = path {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }
}
