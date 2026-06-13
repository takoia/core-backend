//! Permanent memory (ICM) management: org-wide stats, per-topic map, purge.

use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// `GET /api/memory/overview` — global ICM stats + per-topic counts.
pub async fn overview(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let stats = state.memory.stats().await;
    let topics = state.memory.topics().await;
    Ok(Json(json!({ "stats": stats, "topics": topics })))
}

#[derive(Deserialize)]
pub struct TopicQuery {
    pub topic: String,
}

/// `POST /api/memory/purge?topic=...` — forget all memories in a topic.
pub async fn purge(
    State(state): State<AppState>,
    Query(q): Query<TopicQuery>,
) -> AppResult<Json<Value>> {
    state
        .memory
        .forget_topic(&q.topic)
        .await
        .map_err(crate::error::AppError::Other)?;
    Ok(Json(json!({ "ok": true, "purged": q.topic })))
}
