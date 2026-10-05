//! Permanent memory (ICM) management: org-wide stats, per-topic map, purge.

use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// `GET /api/memory/overview` — global memory stats + per-topic counts, from
/// ICM or, on a host without the `icm` binary (`icm_available: false`), from
/// the server's own database.
pub async fn overview(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
) -> AppResult<Json<Value>> {
    crate::http::users::require_admin(&me)?;
    let stats = state.memory.stats().await;
    let topics = state.memory.topics().await;
    Ok(Json(json!({
        "stats": stats,
        "topics": topics,
        "icm_available": state.memory.icm_available(),
    })))
}

#[derive(Deserialize)]
pub struct TopicQuery {
    pub topic: String,
}

/// `POST /api/memory/purge?topic=...` — forget all memories in a topic. The
/// knowledge topic (`takoia/know/<agent id>`) is rebuilt afterwards from the
/// agent's episodes; the owner topic takes the knowledge along for good.
pub async fn purge(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Query(q): Query<TopicQuery>,
) -> AppResult<Json<Value>> {
    crate::http::users::require_admin(&me)?;
    let Some(scope) = crate::memory::MemoryScope::parse_topic(&q.topic) else {
        return Err(crate::error::AppError::BadRequest(
            "topic must be takoia/agent/<agent id>, takoia/know/<agent id> or \
             takoia/fork/<agent id>/<account>"
                .into(),
        ));
    };
    let owned: Option<(String,)> =
        sqlx::query_as("SELECT id FROM agents WHERE id = ? AND account_id = ?")
            .bind(scope.agent_id())
            .bind(&me.account_id)
            .fetch_optional(&state.db)
            .await?;
    if owned.is_none() {
        return Err(crate::error::AppError::NotFound("agent not found".into()));
    }
    let purged = state
        .memory
        .forget(&scope)
        .await
        .map_err(crate::error::AppError::Other)?;
    // `complete` is false when ICM did not confirm: its entries are still
    // recalled, and the purge must be asked again. `requeued` counts the
    // episodes a purge of the knowledge topic alone sent back to be distilled:
    // the layer is rebuilt from them by the next passes.
    Ok(Json(json!({
        "ok": true,
        "purged": q.topic,
        "icm_failed": purged.icm_failed,
        "complete": purged.icm_failed == 0,
        "requeued": purged.requeued,
    })))
}
