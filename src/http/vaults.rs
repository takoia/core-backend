//! Settings: pluggable secret-backend selection for connector / AI-tool secrets
//! (local encrypted storage, or external HashiCorp Vault / Azure Key Vault /
//! GCP Secret Manager / AWS Secrets Manager).

use crate::error::{AppError, AppResult};
use crate::http::users::{CurrentUser, User};
use crate::secrets::{BackendConfig, SecretManager, BACKENDS, SENSITIVE_KEYS};
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

/// Mask the values of sensitive params so they are never sent to the client.
fn mask(params: &Value) -> Value {
    let mut out = params.clone();
    if let Some(obj) = out.as_object_mut() {
        for k in SENSITIVE_KEYS {
            if let Some(v) = obj.get_mut(k) {
                let set = v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false);
                *v = json!(if set { "********" } else { "" });
            }
        }
    }
    out
}

/// `GET /api/settings/secret-backend` — current backend + (masked) config.
pub async fn get_backend(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
) -> AppResult<Json<Value>> {
    ensure_admin(&me)?;
    let cfg = SecretManager::new(&state.cipher, &state.db).active().await;
    Ok(Json(json!({
        "kind": cfg.kind,
        "params": mask(&cfg.params),
        "backends": BACKENDS,
    })))
}

#[derive(Deserialize)]
pub struct SetBackend {
    pub kind: String,
    #[serde(default)]
    pub params: Value,
}

/// `PUT /api/settings/secret-backend` — select + configure the backend.
pub async fn set_backend(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Json(body): Json<SetBackend>,
) -> AppResult<Json<Value>> {
    ensure_admin(&me)?;
    let params = if body.params.is_object() {
        body.params
    } else {
        json!({})
    };
    SecretManager::new(&state.cipher, &state.db)
        .set_active(&body.kind, &params)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "ok": true, "kind": body.kind })))
}

/// `POST /api/settings/secret-backend/test` — probe a candidate backend config.
/// Blank sensitive params fall back to the stored ones so a masked token works.
pub async fn test_backend(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Json(body): Json<SetBackend>,
) -> AppResult<Json<Value>> {
    ensure_admin(&me)?;
    let sm = SecretManager::new(&state.cipher, &state.db);
    let mut params = if body.params.is_object() {
        body.params.clone()
    } else {
        json!({})
    };
    let stored = sm.active().await;
    if let (Some(obj), Some(old)) = (params.as_object_mut(), stored.params.as_object()) {
        for k in SENSITIVE_KEYS {
            let empty = obj
                .get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().is_empty())
                .unwrap_or(true);
            if empty {
                if let Some(prev) = old.get(k) {
                    obj.insert(k.to_string(), prev.clone());
                }
            }
        }
    }
    let cfg = BackendConfig {
        kind: body.kind.clone(),
        params,
    };
    match sm.test(&cfg).await {
        Ok(message) => Ok(Json(json!({ "ok": true, "message": message }))),
        Err(e) => Ok(Json(json!({ "ok": false, "message": e.to_string() }))),
    }
}
