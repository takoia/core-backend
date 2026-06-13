# TakoIA — core-backend

**TakoIA turns a professional goal into a finished deliverable, on its own.**
You set an objective, the agent runs an explicit four-step loop —
**analyse → decision → action → restitution** — and hands back the result.
Every step is customizable (system prompt, allowed tools, autonomy level), the
agent keeps a permanent memory, and token usage is metered for usage-based
billing. `core-backend` is the brain: REST API, a PostgreSQL/SQLite job queue,
sandboxed execution, and it also serves the frontend as a single binary.

> Status: **P0 foundations** ✅ — server boots, `/api/health` answers, migrations
> applied, frontend served. The live 4-step agent run (P1) comes next.

## Architecture

```
┌──────────────┐   POST /api/objectives    ┌───────────────────────────────┐
│  Svelte UI   │ ────────────────────────▶ │  axum REST API  (/api)        │
│ (served by   │ ◀──── SSE /jobs/:id/events │  + static frontend (ServeDir) │
│  the binary) │                            └───────────────┬───────────────┘
└──────────────┘                                            │ enqueue
                                                            ▼
                              ┌──────────────────────────────────────────┐
                              │  job queue (SQLite / Postgres)            │
                              └───────────────┬──────────────────────────┘
                                              │ worker dequeues
                                              ▼
                ┌──────────────────────────────────────────────────────┐
                │  agent engine — the 4-step loop                       │
                │  analyse ▶ decision ▶ action ▶ restitution            │
                │   reads memory   │ approval gate │  tools  │ report   │
                └─────────┬─────────────┬──────────────┬────────────────┘
                          │             │              │
                    LlmProvider   approvals (HITL)  ToolExecutor
                  (OpenAI-compat:  pause job until   web_search,
                   claude_max,     human approves    write_report,
                   ollama, gemini,                   send_discord
                   codex)
```

## Stack

- **Rust** (edition 2021), **tokio**, **axum** + tower-http
- **sqlx** with **SQLite** by default (single binary, zero infra); Postgres 18
  is a drop-in via `DATABASE_URL`
- **reqwest** for LLM providers (OpenAI-compatible) and connectors
- **SSE** for live job progress
- **chacha20poly1305** to encrypt connector credentials at rest
- Frontend: **Svelte + Vite**, built with **Bun**, served by the binary

## Setup (3 commands)

```bash
make setup        # install frontend deps + create .env (with a fresh MASTER_KEY)
make build        # build the frontend bundle + the release binary
make run          # serve on http://localhost:8080
```

Then open <http://localhost:8080>.

> SQLite needs no container. To use Postgres instead: `docker compose up -d`
> then set `DATABASE_URL=postgres://takoia:takoia@localhost:5432/takoia`.

## Development with hot reload

```bash
make dev          # backend auto-reload (cargo-watch) + frontend HMR together
```

Open <http://localhost:5173> — the Vite dev server has hot module reload and
proxies `/api` (and SSE) to the backend on `:8080`, so the app is same-origin.
Edit a `.svelte` file and the browser updates instantly; edit Rust and the
backend recompiles and restarts.

Two-terminal variant: `make dev-api` and `make dev-web`.

## Configuration

All config is environment-driven (see `.env-sample`). LLM providers are
configured (encrypted) from the Settings UI; the `*_BASE_URL/_API_KEY/_MODEL`
vars only **seed** the `connectors` table on first boot so the demo runs without
opening the UI. The four supported providers are all OpenAI-compatible:
`claude_max` (plan proxy), `ollama` (local, offline-safe fallback), `gemini`,
`codex`.

## How the contest criteria map

1. **Relevance** — the agent runs a real pro task end-to-end (tech-watch demo).
2. **Demo quality** — single binary, live SSE timeline of the 4 steps.
3. **Market fit** — usage metering (`token_usage`), encrypted connectors, an
   adjustable autonomy level (`full_auto` vs `confirm_before_action`).
4. **Originality** — per-step customizable prompts/tools + human-in-the-loop.

## Tests

```bash
make test
```
