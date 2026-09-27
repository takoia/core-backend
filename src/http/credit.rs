//! Prepaid credit endpoints. The ledger is the source of truth; a payment
//! provider, when wired, will write `topup` rows through the same function.

use crate::error::{AppError, AppResult};
use crate::http::users::{require_admin, CurrentUser};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// `GET /api/credit` — the caller's account: balance, holds in flight,
/// available headroom, and the most recent ledger rows.
pub async fn get(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
) -> AppResult<Json<Value>> {
    let balance = crate::billing::balance(&state.db, &me.account_id)
        .await
        .map_err(AppError::Other)?;
    #[derive(serde::Serialize, sqlx::FromRow)]
    struct Row {
        delta_usd: f64,
        reason: String,
        job_id: Option<String>,
        created_at: String,
    }
    let ledger = sqlx::query_as::<_, Row>(
        "SELECT delta_usd, reason, job_id, created_at FROM credit_ledger
         WHERE account_id = ? ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&me.account_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "balance": balance, "ledger": ledger })))
}

#[derive(Deserialize)]
pub struct TopupInput {
    pub delta_usd: f64,
    /// `topup` (default), `refund` or `adjustment`.
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /api/accounts/:id/credit` — add or adjust credit (admin). The account
/// must exist; the delta may be negative for a correction.
pub async fn topup(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path(account_id): Path<String>,
    Json(body): Json<TopupInput>,
) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    if !body.delta_usd.is_finite() || body.delta_usd == 0.0 {
        return Err(AppError::BadRequest(
            "delta_usd must be a non-zero number".into(),
        ));
    }
    let reason = body.reason.as_deref().unwrap_or("topup");
    if !matches!(reason, "topup" | "refund" | "adjustment") {
        return Err(AppError::BadRequest(
            "reason must be topup, refund or adjustment".into(),
        ));
    }
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = ?")
        .bind(&account_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("account not found".into()));
    }
    let balance = crate::billing::topup(&state.db, &account_id, body.delta_usd, reason)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(
        json!({ "account_id": account_id, "balance": balance }),
    ))
}
