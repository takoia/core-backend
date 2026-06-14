# TakoIA — Backlog & Roadmap

Status snapshot. `[x]` = shipped to the live VM, `[ ]` = open. Grouped by priority.

## Shipped (deployed today)

- [x] Concurrent job worker (`Semaphore(4)`) — manual tests no longer queue behind perpetual loop jobs.
- [x] `claude -p` hard timeout (300s) + `kill_on_drop` — a hung CLI no longer pins a worker.
- [x] Token accounting: include `cache_creation_input_tokens` + `cache_read_input_tokens` (was under-billing).
- [x] Memory `resync_mirror_from_icm` made transactional — can no longer wipe/corrupt a healthy mirror.
- [x] Memory `recall` falls back to `icm list --sort weight` when keyword recall misses, with a sentinel guard so `"No memories found."` is never injected as memory.
- [x] Cleaned the 3 corrupted `consolidated` mirror rows (TOON-header garbage).
- [x] Engine resume guards: skip tool gathering + Discord re-send when the step was already done (no re-billing / no duplicate alerts).
- [x] Action-step prompt hardened: the agent may no longer claim to create crons/files/configs it cannot execute (stops the hallucinated "6 crons active").
- [x] README rewrite + `.env.example`.
- [x] ICM rebuilt with `json`/`toml`/`toon` list formats (VM).

## P0 — critical, open

- [ ] **Auth on `/api/*`** — `session_token` is issued but never validated; every route is unauthenticated. Add an Axum auth layer + constant-time password compare (`subtle`). Do AFTER the demo video (would break access during filming).
- [ ] **`recover_orphans` hijacks marketplace inline jobs** — on restart it requeues `running` rows, including synchronous `invoke` jobs, which then re-run via the worker with `read_only_memory = false` → pollutes the publisher's curated memory + the re-run is never billed. Give inline jobs a distinct status/flag and exclude them from recovery. (`queue.rs:86`, `marketplace.rs:206`)

## P1 — important, open

- [ ] **Resumed runs double-write** `run-summary` memory + double-increment `runs_count` — gate both behind `restitution_was_done` (value already computed). (`engine.rs:202`, `:546`)
- [ ] **`invoke` mishandles `ConfirmBeforeAction`** — returns 200 with empty output and leaves the job stuck `awaiting_approval`. Match on `RunOutcome` and return 409/422. (`marketplace.rs:220`)
- [ ] **Choreography fan-out not idempotent** — a resumed emitter dispatches downstream jobs twice. De-dup on `(job_id, event)`. (`agent/choreography.rs:46`)
- [ ] **Scheduler re-fires every tick** — the active-job guard `continue`s without advancing `next_run_at`, collapsing interval/cron to "run again immediately after the previous run". Advance `next_run_at` even when skipping. (`scheduler.rs:55`)
- [ ] **`video.rs` + `scaffold` `claude -p` have no timeout** — can hang the HTTP handler forever. Dedupe into one `claude_cli` helper that carries the 300s timeout. (`video.rs:108`, `agents.rs:356`)
- [ ] Malformed cron silently degrades to hourly — validate `cron_expr` at insert, reject 5-field Unix cron. (`schedules.rs:53`)

## Memory moat (personalization quality)

- [x] **Evolving persona (manual trigger)** — `POST /api/agents/:id/evolve-persona` rewrites the persona from accumulated memories (incl. interaction tone). Tested: caractere01 (treated harshly) → guarded/terse; caractere02 (treated kindly) → warm/proactive, from the same neutral start. SHIPPED.
- [x] **Auto interaction-capture** — every run now stores the user's wording as an `interaction` memory (engine.rs), so the agent accumulates the relationship tone automatically. SHIPPED.
- [x] **"Evolve persona" UI button** — `🧬 Faire évoluer le persona` in the builder properties calls the endpoint and updates the persona live. SHIPPED.
- [ ] **Evolving persona — fully automatic** — remaining: call evolve-persona during the consolidation pass (needs the maintenance loop to access config for the claude call; factor the evolve logic out of the HTTP handler). Then divergence happens with zero clicks. (Today: auto-capture + one button click.)

- [ ] **Content-derived keywords** on `store` instead of generic step names (`analyse`/`decision`/…), which is why keyword recall structurally misses. Add a stable `preferences` keyword that ICM always passes through. (`memory.rs:163`)
- [ ] **Importance tiers** — write user preferences/corrections/demonstrations at `high`/`critical`, run-summaries at `medium`/`low`, so consolidation/decay protect the moat instead of burying it. (`memory.rs:155`, `engine.rs:224`)
- [ ] **`recall_feedback` has the same empty-`memories[0]` header bug** recall had — add the `toon_has_entries` guard. (`memory.rs:228`)
- [ ] **Blend recall**: query-scoped hits + top-weighted high-importance, deduped, truncate on entry boundaries (not blind `chars().take()`).
- [ ] **Serialize maintenance vs live runs** (per-agent lock) and exclude `high`/`critical` items from LLM consolidation merges. The 30-min maintenance loop now races the concurrent worker on the same ICM topic.
- [ ] **`icm bench-agent` / `bench-recall`** harness — prove ICM-personalized vs vanilla and catch recall regressions in CI. (`read_only_memory` flag already gives a clean recall-only mode.)

## Optimization

- [ ] **Cache the ICM recall once per run** — engine recalls at job start + every step with identical args (~5 recalls × up to 2 `icm` procs = ~10 subprocess spawns/run). Store once in `RunCtx`. (`engine.rs:321`)
- [ ] **Stop re-injecting full 4000-char memory + growing session every step** — input-token blowup on Decision/Action/Restitution.
- [ ] `ensure_isolated_workdir` writes the steering `CLAUDE.md` on every call (blocking fs in async) — write once. (`claude_cli.rs:100`)
- [ ] Batch `event_log` writes (one `tokio::spawn` + INSERT per event today). (`events.rs:60`)
- [ ] Frontend: drop the 2.5s `getJob` poll that overlaps SSE; narrow the canvas `$effect`. (`BuilderView.svelte:536`)
- [ ] `Cargo.toml` `[profile.release]` (lto/codegen-units); run the built binary in prod instead of `cargo-watch`.
- [ ] `event_log` `message LIKE '%..%'` is a full scan — add composite index / FTS. (`logs.rs:53`)

## Code quality

- [ ] Delete dead code: `bootstrap::_unused_seeds` (~250 lines), `KNOWN_TOOLS`, `is_sensitive`, `Memory::count`, `step_with_extra`'s unused `_extra` param.
- [ ] Dedupe: 3× `claude -p` builders, 10× `Command::new("icm")` builders, 12× `strftime(...)` timestamp literal.
- [ ] `u32 + u32 + u32` token sum → `saturating_add` (debug-panic / Zero-Panic rule). (`claude_cli.rs:180`)
- [ ] French leaks in committed files: `claude_cli.rs:3` "forfait", `CLAUDE.md` "Décision" accent.
- [ ] `CLAUDE.md` claims PostgreSQL but the code is SQLite (`?` placeholders, `strftime`) — fix the doc or plan a real migration.

## Features requested

- [ ] **Server backup / restore** — agents (definitions + step configs) and memory (ICM db + DB mirror). Exportable + importable; ideally a scheduled snapshot of `data/takoia.db` + `data/icm.db` + a per-agent TOML export.
- [ ] **Audit the builder toolbox** — review the component palette: are any components missing? Does each component expose enough input/output handles (buttons) for real flows?
- [ ] **Chat page** — pick a permanent agent and ask it a question directly (conversational front door to a persistent agent, using its accumulated memory).
- [ ] **Hide `claude -p` from user-facing video-analysis messages** — the UI leaks the implementation detail (`claude -p`) in the comments shown during video analysis; replace with a neutral label.
- [x] **License** — `LICENSE` added: Business Source License 1.1, Change Date 2028-06-14 → Apache 2.0. `license = "BUSL-1.1"` in Cargo.toml, License section in README. (Apply the same to the other repos if desired.)
- [ ] **"Duplicate agent" button — with or without memory** — clone an agent's definition (steps, persona, tools). Two modes: (a) fresh clone = same definition, empty memory; (b) clone + copy ICM memory. Directly demonstrates the moat: identical definition + different accumulated memory = two distinct, non-interchangeable agents.
- [ ] **GitHub repo metadata** — set description + topics (gh CLI auth currently broken; commands + values provided separately).
- [x] Admin password configurable via env (`ADMIN_USERNAME`/`ADMIN_PASSWORD`, already set in `.env`) — documented in README.

## Infra / security

- [x] fail2ban on SSH/HTTP (sshd jail active).
- [ ] Front `/api` with auth before any public exposure (see P0).
