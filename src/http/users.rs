//! Multi-user authentication (argon2 + bearer-token sessions) and per-agent
//! RBAC (owner / editor / viewer). Users live in the single org account; the
//! `agent_permissions` table controls who can view/edit/run/delete each agent.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

/// Roles, most-privileged first. owner > editor > viewer.
pub const ROLES: [&str; 3] = ["owner", "editor", "viewer"];

fn role_rank(role: &str) -> i32 {
    match role {
        "owner" => 3,
        "editor" => 2,
        "viewer" => 1,
        _ => 0,
    }
}

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))?
        .to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default().verify_password(pw.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

fn new_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use rand::RngCore;
    let mut buf = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

#[derive(sqlx::FromRow, Clone)]
pub struct User {
    pub id: String,
    pub account_id: String,
    pub email: String,
    pub name: String,
    pub is_admin: i64,
}

impl User {
    fn to_json(&self) -> Value {
        json!({ "id": self.id, "email": self.email, "name": self.name, "is_admin": self.is_admin != 0 })
    }
}

/// Create the first admin user from the env credentials on a fresh install, so
/// login keeps working after the multi-user migration.
pub async fn ensure_admin_user(state: &AppState) -> anyhow::Result<()> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await?;
    if count.0 > 0 {
        return Ok(());
    }
    // Only seed an admin when ADMIN_PASSWORD was explicitly provided. Otherwise
    // leave the users table empty so the first-run setup wizard creates the
    // first admin — no shared default password, secure by design.
    let Some(password) = &state.config.admin_password else {
        tracing::info!("no users and no ADMIN_PASSWORD set; first-run setup wizard will create the admin");
        return Ok(());
    };
    let hash = hash_password(password)?;
    sqlx::query(
        "INSERT INTO users (id, account_id, email, name, password_hash, is_admin) VALUES (?, ?, ?, 'Admin', ?, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(&state.config.admin_username)
    .bind(&hash)
    .execute(&state.db)
    .await?;
    tracing::info!(email = %state.config.admin_username, "seeded initial admin user");
    Ok(())
}

/// `GET /api/setup/status` — public: whether the instance still needs first-run
/// setup (no users yet). The login page shows the create-admin wizard when true.
pub async fn setup_status(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "needs_setup": count.0 == 0 })))
}

#[derive(Deserialize)]
pub struct SetupInput {
    pub email: String,
    #[serde(default)]
    pub name: String,
    pub password: String,
}

/// `POST /api/setup` — create the first admin account on a fresh instance and
/// return a session token (auto-login). Refuses once any user exists.
pub async fn setup(
    State(state): State<AppState>,
    Json(body): Json<SetupInput>,
) -> AppResult<Json<Value>> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await?;
    if count.0 > 0 {
        return Err(AppError::Forbidden("setup already completed".into()));
    }
    if body.email.trim().is_empty() || body.password.is_empty() {
        return Err(AppError::BadRequest("email and password are required".into()));
    }
    let uid = Uuid::new_v4().to_string();
    let name = if body.name.trim().is_empty() {
        "Admin"
    } else {
        body.name.trim()
    };
    let hash = hash_password(&body.password).map_err(AppError::Other)?;
    sqlx::query(
        "INSERT INTO users (id, account_id, email, name, password_hash, is_admin) VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(&uid)
    .bind(DEFAULT_ACCOUNT_ID)
    .bind(body.email.trim())
    .bind(name)
    .bind(&hash)
    .execute(&state.db)
    .await?;
    let token = new_token();
    sqlx::query("INSERT INTO user_sessions (token, user_id, expires_at) VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now','+7 days'))")
        .bind(&token)
        .bind(&uid)
        .execute(&state.db)
        .await?;
    let user: User =
        sqlx::query_as("SELECT id, account_id, email, name, is_admin FROM users WHERE id = ?")
            .bind(&uid)
            .fetch_one(&state.db)
            .await?;
    tracing::info!(email = %body.email.trim(), "first-run setup created the admin user");
    Ok(Json(json!({ "token": token, "username": user.email, "user": user.to_json() })))
}

/// Extractor: the authenticated user behind the request's bearer token.
pub struct CurrentUser(pub User);

#[axum::async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(|s| s.trim().to_string())
            .ok_or_else(|| AppError::Unauthorized("missing bearer token".into()))?;
        let user: Option<User> = sqlx::query_as(
            "SELECT u.id, u.account_id, u.email, u.name, u.is_admin
             FROM user_sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token = ?
               AND (s.expires_at IS NULL OR s.expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        )
        .bind(&token)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| AppError::Other(e.into()))?;
        user.map(CurrentUser)
            .ok_or_else(|| AppError::Unauthorized("invalid or expired session".into()))
    }
}

/// Routes that are reachable WITHOUT a user session: the login/setup flow, the
/// health check, and the key-authenticated public API (`/v1/*` invoke/chat/models
/// + the marketplace catalog + inbound webhooks, which authenticate by their own
/// means). The path is matched with or without the `/api` nest prefix.
fn is_public_path(path: &str) -> bool {
    let p = path.strip_prefix("/api").unwrap_or(path);
    matches!(p, "/health" | "/setup" | "/setup/status" | "/login" | "/marketplace")
        || p.starts_with("/v1/")
        || p.starts_with("/webhooks/")
}

/// Global authentication gate for the `/api` surface: every route requires a
/// valid, unexpired session bearer token except the explicitly public ones.
/// This is the security boundary — individual handlers add RBAC on top.
pub async fn require_session(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if is_public_path(req.uri().path()) {
        return next.run(req).await;
    }
    let token = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string());
    if let Some(token) = token {
        let valid: Option<(String,)> = sqlx::query_as(
            "SELECT user_id FROM user_sessions
             WHERE token = ?
               AND (expires_at IS NULL OR expires_at > strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        )
        .bind(&token)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        if valid.is_some() {
            return next.run(req).await;
        }
    }
    AppError::Unauthorized("authentication required".into()).into_response()
}

#[derive(Deserialize)]
pub struct LoginInput {
    #[serde(alias = "username")]
    pub email: String,
    pub password: String,
}

/// `POST /api/login` — verify credentials, open a bearer session. Auto-bans the
/// client IP after too many failed attempts (brute-force protection).
pub async fn login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginInput>,
) -> AppResult<Json<Value>> {
    let ip = crate::security::client_ip(&headers);
    let sec = crate::security::settings(&state.db).await;
    if sec.auto_ban_enabled && crate::security::is_banned(&state.db, &ip).await {
        return Err(AppError::Forbidden(
            "too many failed attempts — temporarily blocked, try again later".into(),
        ));
    }

    let row: Option<(String, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = ?")
            .bind(body.email.trim())
            .fetch_optional(&state.db)
            .await?;
    let bad = match &row {
        Some((_, hash)) => !verify_password(&body.password, hash),
        None => true,
    };
    if bad {
        crate::security::record_failure(&state.db, &ip, &sec).await;
        return Err(AppError::BadRequest("invalid credentials".into()));
    }
    let (uid, _) = row.expect("checked above");
    crate::security::clear(&state.db, &ip).await;

    let token = new_token();
    sqlx::query("INSERT INTO user_sessions (token, user_id, expires_at) VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now','+7 days'))")
        .bind(&token)
        .bind(&uid)
        .execute(&state.db)
        .await?;
    let user: User =
        sqlx::query_as("SELECT id, account_id, email, name, is_admin FROM users WHERE id = ?")
            .bind(&uid)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(json!({ "token": token, "username": user.email, "user": user.to_json() })))
}

/// `POST /api/logout` — revoke the current bearer token.
pub async fn logout(State(state): State<AppState>, parts: axum::http::HeaderMap) -> AppResult<Json<Value>> {
    if let Some(token) = parts
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        let _ = sqlx::query("DELETE FROM user_sessions WHERE token = ?")
            .bind(token.trim())
            .execute(&state.db)
            .await;
    }
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/me` — the current user.
pub async fn me(CurrentUser(user): CurrentUser) -> AppResult<Json<Value>> {
    Ok(Json(user.to_json()))
}

/// `GET /api/settings/security` — brute-force auto-ban config (admin).
pub async fn get_security(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    let s = crate::security::settings(&state.db).await;
    Ok(Json(json!({
        "auto_ban_enabled": s.auto_ban_enabled,
        "max_failed_attempts": s.max_failed_attempts,
        "window_secs": s.window_secs,
        "ban_secs": s.ban_secs,
    })))
}

#[derive(Deserialize)]
pub struct SecurityInput {
    pub auto_ban_enabled: Option<bool>,
    pub max_failed_attempts: Option<i64>,
    pub window_secs: Option<i64>,
    pub ban_secs: Option<i64>,
}

/// `PUT /api/settings/security` — update the auto-ban config (admin).
pub async fn set_security(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Json(body): Json<SecurityInput>,
) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    let cur = crate::security::settings(&state.db).await;
    let s = crate::security::SecuritySettings {
        auto_ban_enabled: body.auto_ban_enabled.unwrap_or(cur.auto_ban_enabled),
        max_failed_attempts: body.max_failed_attempts.unwrap_or(cur.max_failed_attempts).max(1),
        window_secs: body.window_secs.unwrap_or(cur.window_secs).max(1),
        ban_secs: body.ban_secs.unwrap_or(cur.ban_secs).max(1),
    };
    crate::security::set_settings(&state.db, &s).await.map_err(AppError::Other)?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/users` — list users (org admin only).
pub async fn list_users(State(state): State<AppState>, CurrentUser(me): CurrentUser) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    let users: Vec<User> = sqlx::query_as(
        "SELECT id, account_id, email, name, is_admin FROM users WHERE account_id = ? ORDER BY created_at",
    )
    .bind(&me.account_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "users": users.iter().map(User::to_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
pub struct CreateUser {
    pub email: String,
    #[serde(default)]
    pub name: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
}

/// `POST /api/users` — create a user (org admin only).
pub async fn create_user(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Json(body): Json<CreateUser>,
) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    if body.email.trim().is_empty() || body.password.is_empty() {
        return Err(AppError::BadRequest("email and password are required".into()));
    }
    let id = Uuid::new_v4().to_string();
    let hash = hash_password(&body.password).map_err(AppError::Other)?;
    sqlx::query(
        "INSERT INTO users (id, account_id, email, name, password_hash, is_admin) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&me.account_id)
    .bind(body.email.trim())
    .bind(&body.name)
    .bind(&hash)
    .bind(body.is_admin as i64)
    .execute(&state.db)
    .await
    .map_err(|_| AppError::BadRequest("email already exists".into()))?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct UpdateUser {
    pub name: Option<String>,
    pub password: Option<String>,
    pub is_admin: Option<bool>,
}

/// `PUT /api/users/:id` — update a user (org admin only).
pub async fn update_user(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateUser>,
) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    if let Some(name) = &body.name {
        sqlx::query("UPDATE users SET name = ? WHERE id = ? AND account_id = ?")
            .bind(name)
            .bind(&id)
            .bind(&me.account_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(pw) = &body.password {
        if !pw.is_empty() {
            let hash = hash_password(pw).map_err(AppError::Other)?;
            sqlx::query("UPDATE users SET password_hash = ? WHERE id = ? AND account_id = ?")
                .bind(&hash)
                .bind(&id)
                .bind(&me.account_id)
                .execute(&state.db)
                .await?;
        }
    }
    if let Some(adm) = body.is_admin {
        sqlx::query("UPDATE users SET is_admin = ? WHERE id = ? AND account_id = ?")
            .bind(adm as i64)
            .bind(&id)
            .bind(&me.account_id)
            .execute(&state.db)
            .await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/users/:id` — remove a user (org admin only; not self).
pub async fn delete_user(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    require_admin(&me)?;
    if id == me.id {
        return Err(AppError::BadRequest("you cannot delete your own account".into()));
    }
    sqlx::query("DELETE FROM users WHERE id = ? AND account_id = ?")
        .bind(&id)
        .bind(&me.account_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `GET /api/agents/:id/permissions` — list who can access an agent.
pub async fn list_agent_permissions(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    // Any authenticated org member may read the access list.
    let _ = &me;
    #[derive(sqlx::FromRow)]
    struct Row {
        user_id: String,
        email: String,
        name: String,
        role: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT p.user_id, u.email, u.name, p.role
         FROM agent_permissions p JOIN users u ON u.id = p.user_id
         WHERE p.agent_id = ? ORDER BY p.role",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let perms: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "user_id": r.user_id, "email": r.email, "name": r.name, "role": r.role }))
        .collect();
    Ok(Json(json!({ "permissions": perms })))
}

#[derive(Deserialize)]
pub struct SetPermission {
    pub user_id: String,
    pub role: String,
}

/// `POST /api/agents/:id/permissions` — grant/update a user's role on an agent.
/// Requires the caller to be the agent owner or an org admin.
pub async fn set_agent_permission(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<SetPermission>,
) -> AppResult<Json<Value>> {
    if !ROLES.contains(&body.role.as_str()) {
        return Err(AppError::BadRequest("role must be owner, editor or viewer".into()));
    }
    require_agent_role(&state, &id, &me, "owner").await?;
    sqlx::query(
        "INSERT INTO agent_permissions (agent_id, user_id, role) VALUES (?, ?, ?)
         ON CONFLICT(agent_id, user_id) DO UPDATE SET role = excluded.role",
    )
    .bind(&id)
    .bind(&body.user_id)
    .bind(&body.role)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/agents/:id/permissions/:user_id` — revoke a user's access.
pub async fn remove_agent_permission(
    State(state): State<AppState>,
    CurrentUser(me): CurrentUser,
    Path((id, user_id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_agent_role(&state, &id, &me, "owner").await?;
    sqlx::query("DELETE FROM agent_permissions WHERE agent_id = ? AND user_id = ?")
        .bind(&id)
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ── RBAC helpers ───────────────────────────────────────────────────────────

fn require_admin(user: &User) -> AppResult<()> {
    if user.is_admin != 0 {
        Ok(())
    } else {
        Err(AppError::Forbidden("admin privileges required".into()))
    }
}

/// The user's effective role on an agent ("owner" for org admins), or None.
pub async fn agent_role(state: &AppState, agent_id: &str, user: &User) -> Option<String> {
    if user.is_admin != 0 {
        return Some("owner".to_string());
    }
    sqlx::query_as::<_, (String,)>(
        "SELECT role FROM agent_permissions WHERE agent_id = ? AND user_id = ?",
    )
    .bind(agent_id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .map(|r| r.0)
}

/// Require at least `min` role on the agent, else 403.
pub async fn require_agent_role(
    state: &AppState,
    agent_id: &str,
    user: &User,
    min: &str,
) -> AppResult<()> {
    match agent_role(state, agent_id, user).await {
        Some(role) if role_rank(&role) >= role_rank(min) => Ok(()),
        _ => Err(AppError::Forbidden(format!("{min} role required on this agent"))),
    }
}
