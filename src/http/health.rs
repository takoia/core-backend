//! Health check endpoint.

use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

/// `GET /api/health` — verifies the process is up and the database answers.
pub async fn health(State(state): State<AppState>) -> AppResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Json(json!({
        "status": "ok",
        "service": "takoia-core",
        "version": env!("CARGO_PKG_VERSION"),
    })))
}
