# TakoIA — core-backend

**TakoIA is a marketplace of autonomous AI expert agents.** Each agent is a
specialist you describe once and that then works on its own: it runs an explicit
four-step loop — **Analyse → Decision → Action → Restitution** — and hands back a
finished deliverable. What makes an agent *irreplaceable* is its **persistent,
personalized long-term memory (ICM)**: every run teaches it something, so the
more you use an agent the better and more uniquely yours it becomes. Once an
agent is good, you **publish it to the marketplace** and expose it as a
token-billed hosted API — other people invoke it over HTTP, you keep a revenue
share, and the agent (and its memory) never leave your backend.

Built for the **Euratech 2026 hackathon**. `core-backend` is the brain: a Rust
([Axum](https://github.com/tokio-rs/axum)) orchestrator, a job queue, the ICM
memory bridge, a multi-provider LLM registry, and the marketplace + invoke API.
The companion **Svelte** app (with [SvelteFlow](https://svelteflow.dev/)) is a
visual builder for wiring the four steps. LLM calls go through the **Claude CLI**
(`claude -p`) so the agent runs on your Claude plan instead of per-token API
billing.

---

## Table of contents

- [Architecture](#architecture)
- [Prerequisites](#prerequisites)
- [Installation & configuration](#installation--configuration)
- [Environment variables](#environment-variables)
- [Running](#running)
- [Usage](#usage)
- [Security note](#security-note)

---

## Architecture

```
┌──────────────────────────┐   /api (REST + SSE)   ┌────────────────────────────────┐
│  Svelte + SvelteFlow UI   │ ───────────────────▶ │  Axum HTTP API  (/api/*)        │
│  visual 4-step builder    │ ◀── SSE job events ── │  + serves the built frontend    │
└──────────────────────────┘                       └──────────────┬─────────────────┘
                                                                   │ enqueue objective
                                                                   ▼
                                       ┌──────────────────────────────────────────┐
                                       │  job queue   (SQLite via sqlx)            │
                                       └──────────────┬───────────────────────────┘
                                                      │ worker dequeues
                                                      ▼
                     ┌───────────────────────────────────────────────────────────┐
                     │  agent engine — the 4-step loop                            │
                     │  Analyse ▶ Decision ▶ Action ▶ Restitution                 │
                     │   recall memory │ approval gate │ tools │ persist memory   │
                     └───────┬───────────────┬───────────────┬───────────────────┘
                             │               │               │
                       LLM registry     approvals (HITL)   ICM memory store
                    (claude_max via      pause job until    (per-agent topic;
                     claude -p, ollama,   a human approves   `icm` CLI + DB
                     gemini, codex)                          fallback)
```

- **Backend orchestrator (Axum).** Exposes `/api/*` (REST + Server-Sent Events
  for live job progress) and, in a production build, serves the compiled Svelte
  bundle from the same binary. Source under `src/http/`.
- **Job queue & worker.** Objectives become jobs in the `sqlx`/SQLite queue; a
  background worker claims them and drives the agent loop. The marketplace
  invoke endpoint runs a job inline for a synchronous response.
- **Agent engine (the 4-step loop).** `src/agent/` implements
  Analyse → Decision → Action → Restitution. The Decision step can pause on a
  human-in-the-loop approval gate; the Restitution step persists what was
  learned back into memory.
- **ICM memory.** `src/memory.rs` bridges TakoIA to the **ICM** (`icm` CLI) for
  semantic recall and consolidation, with a local SQLite `memories` table as a
  fallback. Each agent owns its own ICM topic, so its expertise is private and
  cumulative — this is what makes a trained agent irreplaceable.
- **LLM provider registry.** `src/llm/` selects a provider per step. The
  default `claude_max` provider shells out to `claude -p --output-format json`;
  `ollama`, `gemini`, and `codex` are OpenAI-compatible HTTP providers.
- **Marketplace.** `src/http/marketplace.rs` handles publishing an agent,
  issuing consumer API keys (`sk_…`), the token-billed invoke API, revenue
  share, and earnings.

**Stack:** Rust (edition 2021) · tokio · Axum 0.7 + tower-http · sqlx 0.8
(SQLite) · reqwest · SSE · chacha20poly1305 (encrypts connector credentials at
rest). Frontend: Svelte 5 · Vite 5 · `@xyflow/svelte` (SvelteFlow) · Tailwind /
Flowbite, built with Bun.

---

## Prerequisites

| Requirement | Why | Notes |
|---|---|---|
| **Rust** (stable toolchain) | builds and runs the backend | `rustup` recommended |
| **Bun** (or Node 18+) | installs deps & builds the Svelte frontend | `bun` is what the Makefile uses |
| **`claude` CLI**, authenticated | the default `claude_max` LLM provider runs `claude -p` | run `claude` once to log in, or generate a plan token with `claude setup-token` |
| **`icm` CLI** | persistent per-agent long-term memory (semantic recall) | optional but recommended — without it, memory falls back to a local SQLite table |
| **SQLite** | embedded database (queue, agents, memory) | no server needed; the file is created automatically |
| `openssl` | generate the `MASTER_KEY` | already present on most systems |

> The backend needs **no external database server**: SQLite is embedded and
> migrations run automatically on first boot.

---

## Installation & configuration

```bash
git clone <this-repo-url> takoia-core-backend
cd takoia-core-backend
```

> **Demo mode — no Claude, no ICM, no API keys.** TakoIA degrades gracefully:
> without the `claude` CLI, agent runs complete using offline **canned** content;
> without the `icm` CLI, long-term memory falls back to a built-in SQLite store.
> So you can evaluate the **entire** product (the 4-step loop, the builder, the
> marketplace, the memory views) with **zero** LLM setup. Add a Claude token later
> for real model output.

### Quick start — Docker (recommended, no Rust/Bun needed)

The only prerequisite is Docker.

```bash
docker compose up --build      # then open http://localhost:8080
# login: admin / takoia
```

This builds the frontend + backend and runs everything in one container. A
`MASTER_KEY` is generated automatically at startup (set it explicitly in
`docker-compose.yml` to persist encrypted connectors across restarts).

### Quick start — local (Rust + Bun)

```bash
make demo          # = make setup + make build + make run  →  http://localhost:8080
```

`make setup` installs the frontend deps and creates a `.env` from `.env-sample`
with a freshly generated `MASTER_KEY`. `make run` builds the frontend bundle if
it is missing. To do it manually:

```bash
cp .env-sample .env
# generate a key and paste it as MASTER_KEY in .env:
openssl rand -base64 32
```

Open the `.env` and set at least `MASTER_KEY` (and `ADMIN_PASSWORD` for a stable
login — otherwise a random one is printed in the logs on each boot).

---

## Environment variables

All configuration is environment-driven (loaded from `.env`). The table below
documents every variable read by `src/config.rs`.

### Required

| Variable | Default | Description |
|---|---|---|
| `MASTER_KEY` | **none — required** | 32-byte key, **base64-encoded**, used to encrypt connector credentials at rest (chacha20poly1305). The server refuses to start without it. Generate with `openssl rand -base64 32`. Must decode to exactly 32 bytes. |

### Database & storage

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite://data/takoia.db?mode=rwc` | Main SQLite database (job queue, agents, marketplace). The file and its parent dir are created on demand. |
| `ICM_DB_PATH` | `data/icm.db` | Dedicated SQLite database used by the `icm` CLI for agent long-term memory. |
| `AGENT_WORKDIR` | `/tmp/takoia-agent-workspace` | Neutral working directory for the agent's `claude -p` subprocesses, deliberately **outside** the project git tree so the CLI does not pick up the host `CLAUDE.md` or trigger project-scoped ICM recall. |

### Authentication (admin login)

| Variable | Default | Description |
|---|---|---|
| `ADMIN_USERNAME` | `admin` | Username for the UI login page. |
| `ADMIN_PASSWORD` | *(generated)* | Password for the UI login. **IMPORTANT:** if this is unset/empty, a **random password is generated on every boot and printed in the server logs** (look for `admin login -> username: … password: …`). Set it explicitly to get a stable login. |

### Server & frontend

| Variable | Default | Description |
|---|---|---|
| `BIND_ADDR` | `127.0.0.1:8080` | Address the HTTP server binds to. |
| `FRONTEND_DEV_ORIGIN` | `http://localhost:5173` | CORS origin allowed for the Vite dev server during hot-reload. Unused in a same-origin production build. |

### LLM providers

| Variable | Default | Description |
|---|---|---|
| `DEFAULT_LLM_PROVIDER` | `claude_max` | Provider used by the demo agent and as the default for new agents. |
| `CLAUDE_MAX_TOKEN` | *(unset)* | Optional Claude **plan** token from `claude setup-token`, passed to `claude -p` as `CLAUDE_CODE_OAUTH_TOKEN` so the agent consumes your plan instead of API credits. If unset, `claude -p` uses whatever login the `claude` CLI already has. |

**Provider seeds.** Providers are normally configured (and encrypted) from the
Settings UI. The variables below only **seed** the `connectors` table on first
boot, so the demo works without opening the UI. For each provider `<NAME>` in
`{CLAUDE_MAX, OLLAMA, GEMINI, CODEX}`:

| Variable | Description |
|---|---|
| `<NAME>_BASE_URL` | OpenAI-compatible base URL. A seed is created only if this is set. |
| `<NAME>_API_KEY` | API key (optional — e.g. local Ollama needs none). |
| `<NAME>_MODEL` | Default model id for that provider. |

The four logical providers are `claude_max` (Claude plan proxy), `ollama`
(local, offline-safe fallback), `gemini`, and `codex`.

### Integrations & logging

| Variable | Default | Description |
|---|---|---|
| `DISCORD_WEBHOOK_URL` | *(unset)* | Optional Discord webhook for the `send_discord` notification tool. |
| `RUST_LOG` | `takoia=debug,tower_http=debug,info` | Tracing filter. |

### `.env.example`

A ready-to-copy [`.env.example`](./.env.example) lives next to this README. Copy
it to `.env`, then set `MASTER_KEY` and `ADMIN_PASSWORD`:

```dotenv
# ── Required ────────────────────────────────────────────────────────────────
# 32 bytes, base64. Generate with: openssl rand -base64 32
MASTER_KEY=REPLACE_ME_WITH_openssl_rand_base64_32

# ── Auth ────────────────────────────────────────────────────────────────────
# Stable admin login. If ADMIN_PASSWORD is empty, a random one is generated and
# printed in the logs on EVERY boot — set it for a stable login.
ADMIN_USERNAME=admin
ADMIN_PASSWORD=

# ── Server ──────────────────────────────────────────────────────────────────
BIND_ADDR=127.0.0.1:8080
FRONTEND_DEV_ORIGIN=http://localhost:5173

# ── Database & storage ──────────────────────────────────────────────────────
DATABASE_URL=sqlite://data/takoia.db?mode=rwc
ICM_DB_PATH=data/icm.db
AGENT_WORKDIR=/tmp/takoia-agent-workspace

# ── LLM providers ───────────────────────────────────────────────────────────
DEFAULT_LLM_PROVIDER=claude_max
# Optional Claude plan token (claude setup-token) -> CLAUDE_CODE_OAUTH_TOKEN.
CLAUDE_MAX_TOKEN=

# Seed connectors (only used on first boot if the connectors table is empty).
CLAUDE_MAX_BASE_URL=
CLAUDE_MAX_API_KEY=
CLAUDE_MAX_MODEL=

OLLAMA_BASE_URL=http://localhost:11434/v1
OLLAMA_API_KEY=
OLLAMA_MODEL=llama3.1

GEMINI_BASE_URL=https://generativelanguage.googleapis.com/v1beta/openai
GEMINI_API_KEY=
GEMINI_MODEL=gemini-2.0-flash

CODEX_BASE_URL=https://api.openai.com/v1
CODEX_API_KEY=
CODEX_MODEL=gpt-4o-mini

# ── Integrations & logging ──────────────────────────────────────────────────
DISCORD_WEBHOOK_URL=
RUST_LOG=takoia=debug,tower_http=debug,info
```

---

## Running

### Migrations

There is **no manual migration step**: on startup the binary connects to
`DATABASE_URL` and runs all SQL migrations in `migrations/` automatically
(`db::migrate`), then seeds a default account, providers, and demo agents on
first boot. The migrations are managed with `sqlx`; if you have the
[`sqlx` CLI](https://crates.io/crates/sqlx-cli) installed you can also run them
by hand with `sqlx migrate run` (against the same `DATABASE_URL`).

### Backend (production build)

```bash
cargo run --release          # or: make run
```

The server logs `listening` on `BIND_ADDR` (default `127.0.0.1:8080`) and prints
the admin login. With a production frontend bundle present, it serves the app on
the same origin at <http://localhost:8080>.

### Frontend dev server (hot reload)

For UI development, run Vite's dev server alongside the backend:

```bash
# both at once (backend auto-reload + frontend HMR):
make dev

# or in two terminals:
make dev-api                 # cargo watch -x run   (backend on :8080)
make dev-web                 # bun run dev           (Vite on :5173)
```

Open <http://localhost:5173>. **Vite proxies `/api` to the backend on `:8080`**
(see `frontend/vite.config.ts`), so the browser sees a single same-origin app:
edit a `.svelte` file and the page updates instantly; edit Rust and `cargo watch`
recompiles and restarts the server.

### How they connect

- **Dev:** browser → Vite (`:5173`) → proxy `/api` → Axum (`:8080`).
- **Prod:** browser → Axum (`:8080`) serves both the static frontend and `/api`.

### Tests

```bash
cargo test                   # or: make test
```

---

## Usage

### 1. Log in

Open the app and sign in with `ADMIN_USERNAME` / `ADMIN_PASSWORD` (or the
random password from the logs if you left it unset).

### 2. Create / describe an agent

Create an agent and describe what expert it should be (its persona, the system
prompt, the allowed tools, and the autonomy level). You can also scaffold one
from a short description, or import/export an agent as TOML
(`POST /api/agents/import`, `GET /api/agents/:id/export`).

### 3. Build the 4-step loop (visual builder)

In the SvelteFlow builder, wire the four steps —
**Analyse → Decision → Action → Restitution**. Per step you can pick the LLM
provider, the tools, and whether the Decision step requires a human approval
(human-in-the-loop) before the agent takes action. The agent **recalls** its ICM
memory at the start of a run and **persists** what it learned at Restitution, so
it improves with every objective.

### 4. Run an objective

Give the agent an objective (`POST /api/objectives`). It is enqueued as a job;
follow the live timeline over SSE at `GET /api/jobs/:id/events`.

### 5. Publish to the marketplace

Publish an agent (`POST /api/agents/:id/publish`) with a price per 1k output
tokens and a revenue share. It then appears in the marketplace
(`GET /api/marketplace`) and becomes callable over the hosted API.

### 6. Invoke a published agent (token-billed API)

Consumers create an API key (`sk_…`) and call the agent. The run is metered per
output token, the consumer is billed, and the publisher is credited their share.
The agent runs with **read-only memory**, so a caller can never pollute the
publisher's curated expertise.

```bash
curl -X POST http://localhost:8080/api/v1/agents/<AGENT_ID>/invoke \
  -H "Authorization: Bearer sk_your_consumer_key" \
  -H "Content-Type: application/json" \
  -d '{"input": "Summarize this week in Rust async."}'
```

Earnings are visible at `GET /api/marketplace/earnings`; usage at
`GET /api/usage`.

---

## Security note

This is a **hackathon / demo** backend. Apart from the `/api/login` endpoint and
the marketplace invoke endpoint (`/api/v1/agents/:id/invoke`, which requires a
`Bearer sk_…` consumer key), **the `/api/*` surface is currently
unauthenticated**. Do not expose it directly to the public internet — front it
with proper authentication (a reverse proxy with auth, network isolation, etc.)
before deploying anywhere real. Connector credentials are encrypted at rest with
`MASTER_KEY`, so keep that key secret and back it up: losing it makes stored
credentials unrecoverable.

---

## License

TakoIA core-backend is licensed under the **Business Source License 1.1** (BSL
1.1) — see [`LICENSE`](./LICENSE). You may copy, modify and make **non-production**
use of the code freely. On the **Change Date (2028-06-14)** the license
automatically converts to the **Apache License, Version 2.0**.
