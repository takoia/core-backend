//! Brute-force protection: per-IP auto-ban after too many failed logins.
//! Tunable via `security_settings` (enabled by default). Counts failures inside
//! a rolling window; once the threshold is hit the IP is banned for `ban_secs`.

use crate::db::Db;
use axum::http::HeaderMap;

#[derive(Clone, Debug)]
pub struct SecuritySettings {
    pub auto_ban_enabled: bool,
    pub max_failed_attempts: i64,
    pub window_secs: i64,
    pub ban_secs: i64,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            auto_ban_enabled: true,
            max_failed_attempts: 5,
            window_secs: 900,
            ban_secs: 900,
        }
    }
}

pub async fn settings(db: &Db) -> SecuritySettings {
    let row: Option<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT auto_ban_enabled, max_failed_attempts, window_secs, ban_secs FROM security_settings WHERE id = 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    match row {
        Some((e, m, w, b)) => SecuritySettings {
            auto_ban_enabled: e != 0,
            max_failed_attempts: m.max(1),
            window_secs: w.max(1),
            ban_secs: b.max(1),
        },
        None => SecuritySettings::default(),
    }
}

pub async fn set_settings(db: &Db, s: &SecuritySettings) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE security_settings SET auto_ban_enabled = ?, max_failed_attempts = ?, window_secs = ?, ban_secs = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = 1",
    )
    .bind(s.auto_ban_enabled as i64)
    .bind(s.max_failed_attempts.max(1))
    .bind(s.window_secs.max(1))
    .bind(s.ban_secs.max(1))
    .execute(db)
    .await?;
    Ok(())
}

/// Best-effort client IP behind the reverse proxy (Caddy sets X-Forwarded-For).
pub fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// True if this IP is currently banned. Call before verifying credentials.
pub async fn is_banned(db: &Db, ip: &str) -> bool {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT banned_until FROM login_attempts WHERE ip = ?")
            .bind(ip)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    if let Some((Some(until),)) = row {
        let now: (String,) = sqlx::query_as("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')")
            .fetch_one(db)
            .await
            .unwrap_or_else(|_| ("".into(),));
        return until.as_str() > now.0.as_str();
    }
    false
}

/// Record a failed login for this IP; ban it if the threshold is reached.
/// Returns true if the IP just got banned.
pub async fn record_failure(db: &Db, ip: &str, s: &SecuritySettings) -> bool {
    if !s.auto_ban_enabled {
        return false;
    }
    // Reset the window if it has elapsed, then increment.
    let _ = sqlx::query(
        r#"INSERT INTO login_attempts (ip, failed_count, window_start)
           VALUES (?, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
           ON CONFLICT(ip) DO UPDATE SET
             failed_count = CASE
               WHEN window_start IS NULL
                 OR window_start <= strftime('%Y-%m-%dT%H:%M:%fZ','now', ?)
               THEN 1 ELSE failed_count + 1 END,
             window_start = CASE
               WHEN window_start IS NULL
                 OR window_start <= strftime('%Y-%m-%dT%H:%M:%fZ','now', ?)
               THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') ELSE window_start END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(ip)
    .bind(format!("-{} seconds", s.window_secs))
    .bind(format!("-{} seconds", s.window_secs))
    .execute(db)
    .await;

    let count: (i64,) = sqlx::query_as("SELECT failed_count FROM login_attempts WHERE ip = ?")
        .bind(ip)
        .fetch_one(db)
        .await
        .unwrap_or((0,));
    if count.0 >= s.max_failed_attempts {
        let _ = sqlx::query(
            "UPDATE login_attempts SET banned_until = strftime('%Y-%m-%dT%H:%M:%fZ','now', ?) WHERE ip = ?",
        )
        .bind(format!("+{} seconds", s.ban_secs))
        .bind(ip)
        .execute(db)
        .await;
        tracing::warn!(ip, attempts = count.0, "ip auto-banned for repeated failed logins");
        return true;
    }
    false
}

/// Clear an IP's failure state after a successful login.
pub async fn clear(db: &Db, ip: &str) {
    let _ = sqlx::query("DELETE FROM login_attempts WHERE ip = ?")
        .bind(ip)
        .execute(db)
        .await;
}
