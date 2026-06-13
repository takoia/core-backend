//! Login endpoint. The admin password is generated at startup and printed in
//! the server logs. A successful login returns an opaque session token.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

/// `POST /api/login` — validate admin credentials, return a session token.
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginInput>,
) -> AppResult<Json<Value>> {
    let cfg = &state.config;
    if body.username == cfg.admin_username && body.password == cfg.admin_password {
        Ok(Json(json!({
            "token": cfg.session_token,
            "username": cfg.admin_username,
        })))
    } else {
        Err(AppError::BadRequest("invalid credentials".into()))
    }
}
