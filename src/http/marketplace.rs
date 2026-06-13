//! Marketplace: public expert agents others can run (and be billed for).

use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

/// `GET /api/marketplace` — list published (public) expert agents.
pub async fn list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    #[derive(Serialize, sqlx::FromRow)]
    struct PublicAgent {
        id: String,
        name: String,
        description: String,
        expertise_domain: String,
        price_per_run_usd: f64,
        runs_count: i64,
        published_at: Option<String>,
    }
    let rows = sqlx::query_as::<_, PublicAgent>(
        r#"SELECT id, name, description, expertise_domain, price_per_run_usd,
                  runs_count, published_at
           FROM agents WHERE visibility = 'public'
           ORDER BY runs_count DESC, published_at DESC"#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "agents": rows })))
}
