//! Usage metering: token consumption per provider — the billing basis.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

/// `GET /api/usage` — totals per provider plus recent entries.
pub async fn get(State(state): State<AppState>) -> AppResult<Json<Value>> {
    #[derive(Serialize, sqlx::FromRow)]
    struct ProviderTotal {
        provider: String,
        prompt_tokens: i64,
        completion_tokens: i64,
        estimated_cost: f64,
        calls: i64,
    }
    let totals = sqlx::query_as::<_, ProviderTotal>(
        r#"SELECT provider,
                  COALESCE(SUM(prompt_tokens),0)     AS prompt_tokens,
                  COALESCE(SUM(completion_tokens),0) AS completion_tokens,
                  COALESCE(SUM(estimated_cost),0)    AS estimated_cost,
                  COUNT(*)                           AS calls
           FROM token_usage WHERE account_id = ?
           GROUP BY provider ORDER BY estimated_cost DESC"#,
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_all(&state.db)
    .await?;

    #[derive(Serialize, sqlx::FromRow)]
    struct Recent {
        provider: String,
        model: String,
        prompt_tokens: i64,
        completion_tokens: i64,
        estimated_cost: f64,
        created_at: String,
    }
    let recent = sqlx::query_as::<_, Recent>(
        r#"SELECT provider, model, prompt_tokens, completion_tokens, estimated_cost, created_at
           FROM token_usage WHERE account_id = ? ORDER BY created_at DESC LIMIT 50"#,
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_all(&state.db)
    .await?;

    let grand_total: f64 = totals.iter().map(|t| t.estimated_cost).sum();
    Ok(Json(json!({
        "totals": totals,
        "recent": recent,
        "estimated_total_usd": grand_total,
    })))
}
