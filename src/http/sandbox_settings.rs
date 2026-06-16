//! Settings: pluggable execution sandbox for agent runs (none / landlock /
//! bubblewrap / nsjail / docker / podman / firecracker / microsandbox).

use crate::error::{AppError, AppResult};
use crate::http::users::{CurrentUser, User};
use crate::sandbox::{self, SandboxConfig, BACKENDS};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

fn ensure_admin(me: &User) -> AppResult<()> {
    if me.is_admin == 0 {
        return Err(AppError::Forbidden("admin only".into()));
    }
    Ok(())
}

/// `GET /api/settings/sandbox` — current sandbox backend + config.
pub async fn get_sandbox(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
) -> AppResult<Json<Value>> {
    ensure_admin(&me)?;
    let cfg = sandbox::active(&state.db).await;
    Ok(Json(json!({
        "kind": cfg.kind,
        "params": cfg.params,
        "backends": BACKENDS,
    })))
}

#[derive(Deserialize)]
pub struct SetSandbox {
    pub kind: String,
    #[serde(default)]
    pub params: Value,
}

/// `PUT /api/settings/sandbox` — select + configure the sandbox backend.
pub async fn set_sandbox(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Json(body): Json<SetSandbox>,
) -> AppResult<Json<Value>> {
    ensure_admin(&me)?;
    let params = if body.params.is_object() {
        body.params
    } else {
        json!({})
    };
    sandbox::set_active(&state.db, &body.kind, &params)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "ok": true, "kind": body.kind })))
}

/// `POST /api/settings/sandbox/test` — probe whether the host can run a backend.
pub async fn test_sandbox(
    State(_state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Json(body): Json<SetSandbox>,
) -> AppResult<Json<Value>> {
    ensure_admin(&me)?;
    let cfg = SandboxConfig {
        kind: body.kind,
        params: if body.params.is_object() {
            body.params
        } else {
            json!({})
        },
    };
    match sandbox::probe(&cfg).await {
        Ok(message) => Ok(Json(json!({ "ok": true, "message": message }))),
        Err(e) => Ok(Json(json!({ "ok": false, "message": e.to_string() }))),
    }
}
