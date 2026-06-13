//! Enterprise audit logs: query the persisted `event_log` trail with filters
//! (job, kind, free-text message) and pagination.

use crate::error::AppResult;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct LogsQuery {
    pub job_id: Option<String>,
    pub kind: Option<String>,
    /// Free-text search applied to the message column (LIKE %q%).
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
struct LogRow {
    id: String,
    job_id: String,
    kind: String,
    step_type: Option<String>,
    status: Option<String>,
    message: String,
    data: Option<String>,
    created_at: String,
}

/// `GET /api/logs` — paginated audit log with optional filters.
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<LogsQuery>,
) -> AppResult<Json<Value>> {
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let offset = params.offset.unwrap_or(0).max(0);

    // Treat empty strings as "no filter".
    let job_id = params.job_id.filter(|s| !s.is_empty());
    let kind = params.kind.filter(|s| !s.is_empty());
    let q = params.q.filter(|s| !s.is_empty());

    // SQLite NULL-guard pattern: `(? IS NULL OR col = ?)` lets us bind optional
    // filters without dynamic SQL string building.
    let where_clause = r#"
        WHERE (?1 IS NULL OR job_id = ?1)
          AND (?2 IS NULL OR kind = ?2)
          AND (?3 IS NULL OR message LIKE ?3)"#;

    let like = q.as_ref().map(|s| format!("%{s}%"));

    let total: i64 = sqlx::query_as::<_, (i64,)>(&format!(
        "SELECT COUNT(*) FROM event_log {where_clause}"
    ))
    .bind(&job_id)
    .bind(&kind)
    .bind(&like)
    .fetch_one(&state.db)
    .await?
    .0;

    let logs = sqlx::query_as::<_, LogRow>(&format!(
        r#"SELECT id, job_id, kind, step_type, status, message, data, created_at
           FROM event_log {where_clause}
           ORDER BY created_at DESC, id DESC
           LIMIT ?4 OFFSET ?5"#
    ))
    .bind(&job_id)
    .bind(&kind)
    .bind(&like)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({ "logs": logs, "total": total })))
}
