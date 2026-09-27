//! Prepaid credit for marketplace consumers: admission (hold), settlement,
//! release, rate limiting and balance. The pure arithmetic is separate from
//! the database shell so the rules are testable on their own.
//!
//! Invariant: for every account, `SUM(credit_ledger.delta_usd WHERE reason =
//! 'invoke') == -SUM(marketplace_usage.billed_usd)`; both rows are written in
//! the same transaction by [`settle`].

use crate::db::Db;
use anyhow::Result;
use uuid::Uuid;

/// An authenticated consumer API key.
#[derive(Debug, Clone)]
pub struct ConsumerKey {
    pub account_id: String,
    pub api_key_id: String,
    pub rate_limit_per_min: i64,
}

/// Balance view for an account.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Balance {
    pub balance_usd: f64,
    pub held_usd: f64,
    pub available_usd: f64,
    pub max_invoke_usd: f64,
}

/// Worst-case charge of one invoke, used as the reservation: every step may
/// emit up to `max_output_tokens`, capped by the account's per-invoke ceiling.
/// A free agent (price 0) reserves nothing.
pub fn hold_amount(
    price_per_1k: f64,
    max_output_tokens: u32,
    steps: u32,
    max_invoke_usd: f64,
) -> f64 {
    if price_per_1k <= 0.0 {
        return 0.0;
    }
    let worst = (steps as f64 * max_output_tokens as f64 / 1000.0) * price_per_1k;
    worst.min(max_invoke_usd.max(0.0))
}

/// Whether a request may proceed: `recent` calls (holds in flight + settled
/// invokes in the last minute) against the key's limit. A limit <= 0 disables it.
pub fn within_rate_limit(recent: i64, per_min: i64) -> bool {
    per_min <= 0 || recent < per_min
}

/// Current balance, holds and headroom. Missing credit row = zero balance with
/// the default ceiling.
pub async fn balance(db: &Db, account_id: &str) -> Result<Balance> {
    let row: Option<(f64, f64)> = sqlx::query_as(
        "SELECT balance_usd, max_invoke_usd FROM account_credit WHERE account_id = ?",
    )
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    let (balance_usd, max_invoke_usd) = row.unwrap_or((0.0, 5.0));
    let (held_usd,): (f64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount_usd), 0.0) FROM credit_hold WHERE account_id = ?",
    )
    .bind(account_id)
    .fetch_one(db)
    .await?;
    Ok(Balance {
        balance_usd,
        held_usd,
        available_usd: balance_usd - held_usd,
        max_invoke_usd,
    })
}

/// Reserve `amount` for `job_id` if the account's available credit covers it.
/// One transaction: SQLite serialises writers, so two concurrent invokes cannot
/// both pass the check on the same credit. Returns the hold id, or `None` when
/// credit is insufficient. A zero amount always succeeds and still records a
/// hold so the request counts towards the rate limit.
pub async fn place_hold(
    db: &Db,
    key: &ConsumerKey,
    amount: f64,
    job_id: &str,
) -> Result<Option<String>> {
    let mut tx = db.begin().await?;
    let row: Option<(f64,)> =
        sqlx::query_as("SELECT balance_usd FROM account_credit WHERE account_id = ?")
            .bind(&key.account_id)
            .fetch_optional(&mut *tx)
            .await?;
    let balance = row.map(|r| r.0).unwrap_or(0.0);
    let (held,): (f64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount_usd), 0.0) FROM credit_hold WHERE account_id = ?",
    )
    .bind(&key.account_id)
    .fetch_one(&mut *tx)
    .await?;
    if amount > 0.0 && balance - held < amount {
        return Ok(None); // dropping tx rolls back
    }
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO credit_hold (id, account_id, api_key_id, amount_usd, job_id) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&key.account_id)
    .bind(&key.api_key_id)
    .bind(amount)
    .bind(job_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Drop a reservation without charging (the run failed or was refused).
pub async fn release_hold(db: &Db, hold_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM credit_hold WHERE id = ?")
        .bind(hold_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Everything a settled invoke writes, atomically: the usage row, the ledger
/// row (negative delta), the balance decrement, and the hold removal.
pub struct Settlement<'a> {
    pub hold_id: Option<&'a str>,
    pub key: &'a ConsumerKey,
    pub agent_id: &'a str,
    pub publisher_account: &'a str,
    pub job_id: &'a str,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub billed_usd: f64,
    pub publisher_usd: f64,
}

pub async fn settle(db: &Db, s: Settlement<'_>) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query(
        r#"INSERT INTO marketplace_usage
             (id, agent_id, publisher_account, consumer_account, job_id,
              prompt_tokens, completion_tokens, billed_usd, publisher_usd)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(s.agent_id)
    .bind(s.publisher_account)
    .bind(&s.key.account_id)
    .bind(s.job_id)
    .bind(s.prompt_tokens)
    .bind(s.completion_tokens)
    .bind(s.billed_usd)
    .bind(s.publisher_usd)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO credit_ledger (id, account_id, api_key_id, delta_usd, reason, job_id)
         VALUES (?, ?, ?, ?, 'invoke', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&s.key.account_id)
    .bind(&s.key.api_key_id)
    .bind(-s.billed_usd)
    .bind(s.job_id)
    .execute(&mut *tx)
    .await?;
    if s.billed_usd > 0.0 {
        // Create the credit row on first charge so the balance can go (and show)
        // negative if a run exceeded its reservation; the ledger stays exact.
        sqlx::query(
            "INSERT INTO account_credit (account_id, balance_usd) VALUES (?, 0)
             ON CONFLICT(account_id) DO NOTHING",
        )
        .bind(&s.key.account_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE account_credit SET balance_usd = balance_usd - ?,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE account_id = ?",
        )
        .bind(s.billed_usd)
        .bind(&s.key.account_id)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(hold) = s.hold_id {
        sqlx::query("DELETE FROM credit_hold WHERE id = ?")
            .bind(hold)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Add credit (or adjust it) with a ledger row. `reason` is `topup`, `refund`
/// or `adjustment`.
pub async fn topup(db: &Db, account_id: &str, delta_usd: f64, reason: &str) -> Result<Balance> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO account_credit (account_id, balance_usd) VALUES (?, 0)
         ON CONFLICT(account_id) DO NOTHING",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE account_credit SET balance_usd = balance_usd + ?,
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE account_id = ?",
    )
    .bind(delta_usd)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO credit_ledger (id, account_id, delta_usd, reason) VALUES (?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(account_id)
    .bind(delta_usd)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    balance(db, account_id).await
}

/// Calls attributed to a key in the last minute: holds in flight plus settled
/// invokes. Rejected requests create neither and so do not count.
pub async fn recent_calls(db: &Db, api_key_id: &str) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM credit_hold WHERE api_key_id = ?1
              AND created_at > strftime('%Y-%m-%dT%H:%M:%fZ','now','-60 seconds'))
         + (SELECT COUNT(*) FROM credit_ledger WHERE api_key_id = ?1 AND reason = 'invoke'
              AND created_at > strftime('%Y-%m-%dT%H:%M:%fZ','now','-60 seconds'))",
    )
    .bind(api_key_id)
    .fetch_one(db)
    .await?;
    Ok(n)
}

/// Delete holds older than `max_age_secs`: the only safety net when the
/// process died mid-invoke. `0` means every hold (the startup case: none can
/// belong to a live request) — an explicit rule rather than a `<` comparison
/// against "now" that a same-millisecond row would slip through.
pub async fn sweep_stale_holds(db: &Db, max_age_secs: i64) -> Result<u64> {
    let res = if max_age_secs <= 0 {
        sqlx::query("DELETE FROM credit_hold").execute(db).await?
    } else {
        sqlx::query(
            "DELETE FROM credit_hold WHERE created_at < strftime('%Y-%m-%dT%H:%M:%fZ','now', ?)",
        )
        .bind(format!("-{max_age_secs} seconds"))
        .execute(db)
        .await?
    };
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_is_worst_case_capped_by_the_ceiling_and_zero_for_free_agents() {
        // 5 steps x 4096 tokens x 1 USD per 1k = 20.48, capped at 5.
        assert!((hold_amount(1.0, 4096, 5, 5.0) - 5.0).abs() < 1e-9);
        // Cheap agent stays under the ceiling.
        assert!((hold_amount(0.01, 4096, 5, 5.0) - 0.2048).abs() < 1e-9);
        assert_eq!(hold_amount(0.0, 4096, 5, 5.0), 0.0);
        assert_eq!(hold_amount(-1.0, 4096, 5, 5.0), 0.0);
        assert_eq!(
            hold_amount(1.0, 4096, 5, -3.0),
            0.0,
            "negative ceiling clamps to 0"
        );
    }

    #[test]
    fn rate_limit_counts_strictly_and_zero_disables() {
        assert!(within_rate_limit(0, 1));
        assert!(!within_rate_limit(1, 1));
        assert!(within_rate_limit(59, 60));
        assert!(!within_rate_limit(60, 60));
        assert!(within_rate_limit(1_000_000, 0));
    }

    async fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("takoia-billing-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::db::connect(&format!("sqlite://{}/t.db?mode=rwc", dir.display()))
            .await
            .unwrap();
        crate::db::migrate(&pool).await.unwrap();
        sqlx::query("INSERT INTO accounts (id, name) VALUES ('a', 'A')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn key() -> ConsumerKey {
        ConsumerKey {
            account_id: "a".into(),
            api_key_id: "k1".into(),
            rate_limit_per_min: 60,
        }
    }

    #[tokio::test]
    async fn hold_blocks_when_credit_is_short_and_settle_keeps_the_invariant() {
        let db = db().await;
        let k = key();
        assert!(
            place_hold(&db, &k, 1.0, "j0").await.unwrap().is_none(),
            "no credit yet"
        );
        topup(&db, "a", 2.5, "topup").await.unwrap();
        let h1 = place_hold(&db, &k, 1.0, "j1")
            .await
            .unwrap()
            .expect("first hold fits");
        let h2 = place_hold(&db, &k, 1.0, "j2")
            .await
            .unwrap()
            .expect("second hold fits");
        assert!(
            place_hold(&db, &k, 1.0, "j3").await.unwrap().is_none(),
            "2.5 - 2.0 held < 1.0"
        );
        let b = balance(&db, "a").await.unwrap();
        assert!((b.available_usd - 0.5).abs() < 1e-9);

        release_hold(&db, &h2).await.unwrap();
        settle(
            &db,
            Settlement {
                hold_id: Some(&h1),
                key: &k,
                agent_id: "agent",
                publisher_account: "p",
                job_id: "j1",
                prompt_tokens: 100,
                completion_tokens: 700,
                billed_usd: 0.7,
                publisher_usd: 0.49,
            },
        )
        .await
        .unwrap();
        let b = balance(&db, "a").await.unwrap();
        assert!((b.balance_usd - 1.8).abs() < 1e-9);
        assert_eq!(b.held_usd, 0.0);

        let (ledger,): (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(delta_usd), 0.0) FROM credit_ledger WHERE account_id = 'a' AND reason = 'invoke'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        let (usage,): (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(billed_usd), 0.0) FROM marketplace_usage WHERE consumer_account = 'a'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!((ledger + usage).abs() < 1e-9, "ledger and usage agree");
        assert_eq!(recent_calls(&db, "k1").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn stale_holds_are_swept() {
        let db = db().await;
        let k = key();
        topup(&db, "a", 10.0, "topup").await.unwrap();
        place_hold(&db, &k, 1.0, "j1").await.unwrap().unwrap();
        assert_eq!(
            sweep_stale_holds(&db, 3600).await.unwrap(),
            0,
            "fresh hold stays"
        );
        assert_eq!(
            sweep_stale_holds(&db, 0).await.unwrap(),
            1,
            "age-0 sweep clears it"
        );
        assert_eq!(balance(&db, "a").await.unwrap().held_usd, 0.0);
    }
}
