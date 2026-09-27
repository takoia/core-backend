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
    let (balance_usd, max_invoke_usd) = row.unwrap_or((0.0, DEFAULT_MAX_INVOKE_USD));
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

/// What admission needs to know about the call.
pub struct AdmissionRequest<'a> {
    pub key: &'a ConsumerKey,
    pub job_id: &'a str,
    pub price_per_1k: f64,
    pub max_output_tokens: u32,
    /// Steps that may produce billable output (loop steps, web search, nested
    /// call_agent runs).
    pub steps: u32,
    /// The publisher calling their own agent: nothing is reserved, the call
    /// still counts towards the key's rate limit.
    pub self_invoke: bool,
}

#[derive(Debug, PartialEq)]
pub enum Admission {
    Held { hold_id: String, amount_usd: f64 },
    InsufficientCredit { needed_usd: f64, available_usd: f64 },
    RateLimited { per_min: i64 },
}

/// Admit or refuse one call, atomically: rate limit, then credit. One
/// transaction, so concurrent calls on the same key or account cannot all pass
/// on the same headroom. Refusals are recorded as zero-delta `refused` ledger
/// rows so they count towards the rate limit too — a key without credit cannot
/// hammer the endpoint for free.
pub async fn admit(db: &Db, req: AdmissionRequest<'_>) -> Result<Admission> {
    let key = req.key;
    let mut tx = db.begin().await?;

    let (recent,): (i64,) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM credit_hold WHERE api_key_id = ?1
              AND created_at > strftime('%Y-%m-%dT%H:%M:%fZ','now','-60 seconds'))
         + (SELECT COUNT(*) FROM credit_ledger WHERE api_key_id = ?1
              AND reason IN ('invoke', 'refused')
              AND created_at > strftime('%Y-%m-%dT%H:%M:%fZ','now','-60 seconds'))",
    )
    .bind(&key.api_key_id)
    .fetch_one(&mut *tx)
    .await?;
    if !within_rate_limit(recent, key.rate_limit_per_min) {
        record_refusal(&mut tx, key, req.job_id).await?;
        tx.commit().await?;
        return Ok(Admission::RateLimited {
            per_min: key.rate_limit_per_min,
        });
    }

    let credit: Option<(f64, f64)> = sqlx::query_as(
        "SELECT balance_usd, max_invoke_usd FROM account_credit WHERE account_id = ?",
    )
    .bind(&key.account_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (balance, max_invoke) = credit.unwrap_or((0.0, DEFAULT_MAX_INVOKE_USD));
    let (held,): (f64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(amount_usd), 0.0) FROM credit_hold WHERE account_id = ?",
    )
    .bind(&key.account_id)
    .fetch_one(&mut *tx)
    .await?;
    let amount = if req.self_invoke {
        0.0
    } else {
        hold_amount(
            req.price_per_1k,
            req.max_output_tokens,
            req.steps,
            max_invoke,
        )
    };
    let available = balance - held;
    if amount > 0.0 && available < amount {
        record_refusal(&mut tx, key, req.job_id).await?;
        tx.commit().await?;
        return Ok(Admission::InsufficientCredit {
            needed_usd: amount,
            available_usd: available,
        });
    }

    let hold_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO credit_hold (id, account_id, api_key_id, amount_usd, job_id) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&hold_id)
    .bind(&key.account_id)
    .bind(&key.api_key_id)
    .bind(amount)
    .bind(req.job_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Admission::Held {
        hold_id,
        amount_usd: amount,
    })
}

/// Default per-invoke ceiling, also the schema default of account_credit.
const DEFAULT_MAX_INVOKE_USD: f64 = 5.0;

async fn record_refusal(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &ConsumerKey,
    job_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO credit_ledger (id, account_id, api_key_id, delta_usd, reason, job_id)
         VALUES (?, ?, ?, 0.0, 'refused', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&key.account_id)
    .bind(&key.api_key_id)
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// A reservation that releases itself unless it is consumed. Covers every
/// early `?` return and request cancellation between admission and settlement:
/// on drop with the hold still armed, the release is spawned on the runtime.
pub struct HoldGuard {
    db: Db,
    hold_id: Option<String>,
}

impl HoldGuard {
    pub fn new(db: Db, hold_id: String) -> Self {
        Self {
            db,
            hold_id: Some(hold_id),
        }
    }

    /// Hand the hold over to settlement (which deletes it) and disarm the guard.
    pub fn take(mut self) -> String {
        self.hold_id.take().expect("hold taken twice")
    }
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
        if let Some(hold) = self.hold_id.take() {
            let db = self.db.clone();
            tokio::spawn(async move {
                if let Err(e) = release_hold(&db, &hold).await {
                    tracing::warn!(error = %e, hold_id = %hold, "failed to release credit hold");
                }
            });
        }
    }
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

    fn req<'a>(k: &'a ConsumerKey, job: &'a str) -> AdmissionRequest<'a> {
        AdmissionRequest {
            key: k,
            job_id: job,
            // 5 steps x 1000 tokens x 0.2 USD/1k = 1 USD per call.
            price_per_1k: 0.2,
            max_output_tokens: 1000,
            steps: 5,
            self_invoke: false,
        }
    }

    #[tokio::test]
    async fn admission_blocks_when_credit_is_short_and_settle_keeps_the_invariant() {
        let db = db().await;
        let k = key();
        assert!(matches!(
            admit(&db, req(&k, "j0")).await.unwrap(),
            Admission::InsufficientCredit { needed_usd, available_usd }
                if (needed_usd - 1.0).abs() < 1e-9 && available_usd == 0.0
        ));
        topup(&db, "a", 2.5, "topup").await.unwrap();
        let Admission::Held { hold_id: h1, .. } = admit(&db, req(&k, "j1")).await.unwrap() else {
            panic!("first hold fits")
        };
        let Admission::Held { hold_id: h2, .. } = admit(&db, req(&k, "j2")).await.unwrap() else {
            panic!("second hold fits")
        };
        assert!(
            matches!(
                admit(&db, req(&k, "j3")).await.unwrap(),
                Admission::InsufficientCredit { .. }
            ),
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
    }

    #[tokio::test]
    async fn refusals_count_towards_the_rate_limit() {
        let db = db().await;
        let mut k = key();
        k.rate_limit_per_min = 2;
        // Two refusals for lack of credit, then the key is rate limited even
        // after a top-up: a broke key cannot hammer the endpoint for free.
        assert!(matches!(
            admit(&db, req(&k, "j1")).await.unwrap(),
            Admission::InsufficientCredit { .. }
        ));
        assert!(matches!(
            admit(&db, req(&k, "j2")).await.unwrap(),
            Admission::InsufficientCredit { .. }
        ));
        topup(&db, "a", 100.0, "topup").await.unwrap();
        assert!(matches!(
            admit(&db, req(&k, "j3")).await.unwrap(),
            Admission::RateLimited { per_min: 2 }
        ));
        // Self-invokes reserve nothing but are admitted (and counted).
        let mut free = key();
        free.api_key_id = "k2".into();
        let mut r = req(&free, "j4");
        r.self_invoke = true;
        assert!(matches!(
            admit(&db, r).await.unwrap(),
            Admission::Held { amount_usd, .. } if amount_usd == 0.0
        ));
    }

    #[tokio::test]
    async fn dropping_an_armed_guard_releases_the_hold() {
        let db = db().await;
        let k = key();
        topup(&db, "a", 10.0, "topup").await.unwrap();
        let Admission::Held { hold_id, .. } = admit(&db, req(&k, "j1")).await.unwrap() else {
            panic!("admitted")
        };
        {
            let _guard = HoldGuard::new(db.clone(), hold_id);
            assert!((balance(&db, "a").await.unwrap().held_usd - 1.0).abs() < 1e-9);
        }
        // The release is spawned; give the runtime a turn.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(balance(&db, "a").await.unwrap().held_usd, 0.0);
    }

    #[tokio::test]
    async fn stale_holds_are_swept() {
        let db = db().await;
        let k = key();
        topup(&db, "a", 10.0, "topup").await.unwrap();
        assert!(matches!(
            admit(&db, req(&k, "j1")).await.unwrap(),
            Admission::Held { .. }
        ));
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
