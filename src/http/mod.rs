//! HTTP layer: REST API under `/api`, plus static frontend serving with SPA
//! fallback for everything else.

mod agents;
mod approvals;
mod connectors;
mod health;
mod jobs;
mod marketplace;
mod objectives;
mod usage;

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
        // Agents + per-step customization + marketplace publishing
        .route("/agents", get(agents::list).post(agents::create))
        .route("/agents/import", post(agents::import_toml))
        .route("/agents/:id", get(agents::get).put(agents::update))
        .route("/agents/:id/steps", put(agents::update_steps))
        .route("/agents/:id/publish", post(agents::publish))
        .route("/agents/:id/export", get(agents::export_toml))
        .route("/agents/:id/memories", get(agents::memories))
        // Objectives -> jobs
        .route("/objectives", get(objectives::list).post(objectives::create))
        // Jobs + live SSE
        .route("/jobs", get(jobs::list))
        .route("/jobs/:id", get(jobs::get))
        .route("/jobs/:id/events", get(jobs::events))
        // Human-in-the-loop
        .route("/approvals/:id", post(approvals::decide))
        // Settings / connectors (encrypted)
        .route("/connectors", get(connectors::list).post(connectors::upsert))
        .route("/connectors/:id", delete(connectors::delete))
        // Usage metering (billing basis)
        .route("/usage", get(usage::get))
        // Public marketplace
        .route("/marketplace", get(marketplace::list))
        .with_state(state);

    Router::new()
        .nest("/api", api)
        .merge(frontend_service())
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
