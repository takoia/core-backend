//! HTTP layer: REST API under `/api`, plus static frontend serving with SPA
//! fallback for everything else.

mod agents;
mod approvals;
mod connectors;
mod health;
mod integrations;
mod jobs;
mod logs;
mod marketplace;
mod memory;
mod mcp;
mod objectives;
mod schedules;
mod skills;
pub mod users;
mod tts;
mod usage;
mod vaults;
mod video;
mod webhooks;

use crate::state::AppState;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

/// Path to the built frontend bundle, served in production.
const FRONTEND_DIST: &str = "frontend/dist";

/// Build the full application router.
pub fn router(state: AppState) -> Router {
    let cors = build_cors(&state);

    let api = Router::new()
        .route("/health", get(health::health))
        .route("/setup/status", get(users::setup_status))
        .route("/setup", post(users::setup))
        .route("/login", post(users::login))
        .route("/logout", post(users::logout))
        .route("/me", get(users::me))
        // Multi-user management (org admin)
        .route("/users", get(users::list_users).post(users::create_user))
        .route("/users/:id", put(users::update_user).delete(users::delete_user))
        // Agents + per-step customization + marketplace publishing
        .route("/agents", get(agents::list).post(agents::create))
        .route("/agents/import", post(agents::import_toml))
        .route("/agents/import-soul", post(agents::import_soul))
        .route("/agents/scaffold", post(agents::scaffold))
        .route("/agents/:id", get(agents::get).put(agents::update).delete(agents::delete))
        .route("/agents/:id/steps", put(agents::update_steps))
        .route("/agents/:id/publish", post(agents::publish))
        .route("/agents/:id/export", get(agents::export_toml))
        .route("/agents/:id/memories", get(agents::memories))
        .route("/agents/:id/icm-memories", get(agents::icm_memories))
        .route("/agents/:id/memory", post(agents::add_memory))
        .route("/agents/:id/evolve-persona", post(agents::evolve_persona))
        // Per-agent RBAC (owner / editor / viewer)
        .route("/agents/:id/permissions", get(users::list_agent_permissions).post(users::set_agent_permission))
        .route("/agents/:id/permissions/:user_id", delete(users::remove_agent_permission))
        // Objectives -> jobs
        .route("/objectives", get(objectives::list).post(objectives::create))
        // Jobs + live SSE
        .route("/jobs", get(jobs::list))
        .route("/jobs/:id", get(jobs::get))
        .route("/jobs/:id/events", get(jobs::events))
        .route("/jobs/:id/feedback", post(jobs::feedback))
        // Inbound webhooks (e.g. invoice received -> trigger agent)
        .route("/webhooks/:event", post(webhooks::receive))
        // Human-in-the-loop
        .route("/approvals/:id", post(approvals::decide))
        // Recurring schedules (autonomous learning loop)
        .route("/schedules", get(schedules::list).post(schedules::create))
        .route("/schedules/:id", delete(schedules::delete))
        .route("/schedules/:id/toggle", post(schedules::toggle))
        // Settings / connectors (encrypted)
        .route("/connectors", get(connectors::list).post(connectors::upsert))
        .route("/connectors/:id", delete(connectors::delete))
        // Pluggable secret storage backend (local / vault / azure / gcp / aws)
        .route(
            "/settings/secret-backend",
            get(vaults::get_backend).put(vaults::set_backend),
        )
        .route("/settings/secret-backend/test", post(vaults::test_backend))
        // MCP catalog + connect
        .route("/mcp/catalog", get(mcp::catalog))
        .route("/mcp/installed", get(mcp::installed))
        .route("/mcp/connect", post(mcp::connect))
        // Skills catalog + install (from GitHub)
        .route("/skills/catalog", get(skills::catalog))
        .route("/skills/installed", get(skills::installed))
        .route("/skills/install", post(skills::install))
        .route("/skills/github", get(skills::github))
        // Usage metering (billing basis)
        .route("/usage", get(usage::get))
        // Audit logs + external integrations
        .route("/logs", get(logs::list))
        .route("/integrations/email/test", post(integrations::email_test))
        // Permanent memory (ICM) management
        .route("/memory/overview", get(memory::overview))
        .route("/memory/purge", post(memory::purge))
        // Video analysis (frames -> claude -p vision)
        .route("/video/analyze", post(video::analyze))
        // Text-to-speech (OpenAI audio/speech)
        .route("/tts", post(tts::synthesize))
        // Public marketplace
        .route("/marketplace", get(marketplace::list))
        .route("/marketplace/earnings", get(marketplace::earnings))
        .route("/marketplace/usage", get(marketplace::usage))
        // Consumer API keys + public hosted-agent API (token-billed).
        .route("/keys", get(marketplace::list_keys).post(marketplace::create_key))
        .route("/keys/:id", delete(marketplace::revoke_key))
        .route("/v1/agents/:id/invoke", post(marketplace::invoke))
        // OpenAI-compatible API: point any OpenAI SDK base_url at `<host>/api/v1`.
        .route("/v1/chat/completions", post(marketplace::chat_completions))
        .route("/v1/models", get(marketplace::list_models))
        .with_state(state);

    Router::new()
        .nest("/api", api)
        .merge(frontend_service())
        // Video frame uploads (downscaled frames as base64) exceed the 2 MB
        // default; allow large request bodies.
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024 * 1024))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

/// Allow the Vite dev server origin so the frontend hot-reloads against the API.
fn build_cors(state: &AppState) -> CorsLayer {
    let origin = state.config.frontend_dev_origin.clone();
    match origin.parse::<HeaderValue>() {
        Ok(value) => CorsLayer::new()
            .allow_origin(value)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers(tower_http::cors::Any),
        Err(_) => CorsLayer::new(),
    }
}

/// Serve the built frontend: real files from `frontend/dist/assets`, and the
/// SPA `index.html` (HTTP 200) for every other unmatched path so client-side
/// routing and deep links work.
fn frontend_service() -> Router {
    let assets = PathBuf::from(FRONTEND_DIST).join("assets");
    Router::new()
        .nest_service("/assets", ServeDir::new(assets))
        .fallback(get(spa_index))
}

/// Return the SPA shell with a 200 status (the app boots and routes client-side).
async fn spa_index() -> impl IntoResponse {
    let index = PathBuf::from(FRONTEND_DIST).join("index.html");
    match tokio::fs::read_to_string(&index).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "frontend not built — run `bun run build` in frontend/",
        )
            .into_response(),
    }
}
