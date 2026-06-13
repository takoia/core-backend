# CLAUDE.md - TakoIA Core Backend

## Project Overview
`core-backend` is the orchestrator of the TakoIA ecosystem. Written in Rust for maximum reliability and speed, it manages the persistent task queue (PostgreSQL), handles multi-LLM routing, controls execution sandboxing, and talks to MCP servers to execute complex end-to-end workflows.

## Tech Stack & Architecture
- **Language:** Rust (Stable)
- **Database:** PostgreSQL with `sqlx` (Async & compile-time checked queries)
- **API Layer:** Axum (REST & WebSockets for live logging)
- **LLM Routing:** Dynamic orchestration (e.g., fast model for Analysis, powerful model for Decision)
- **State Management:** Tracking the 4 core steps: `Analyse`, `Décision`, `Action`, `Restitution`.

## Development Commands
- Build project: `cargo build`
- Run local server: `cargo run`
- Run tests: `cargo test`
- Database migration: `sqlx migrate run`

## Code Guidelines
1. **Zero Panic:** Never use `unwrap()` or `panic!` in production paths. Propagate errors gracefully using `Result` and `thiserror` / `anyhow`.
2. **Concurrency:** Leverage Tokio async patterns for tasks pulling. Ensure database connections are pooled properly.
3. **Audit Trails:** Every state transition (especially moving to `Action`) must log the decision rationale in the DB for auditability.
4. **Environment Variables:** All secrets (Database URL, LLM API keys) must be loaded from `.env` locally and prepared for Kubernetes Secrets injection.
