# TakoIA — Roadmap

Replaces the old `TODO.md`, which had drifted: everything it listed as P0/P1 was
already fixed. This file tracks what still separates the product from its
pitch — an expert agent whose memory makes it uniquely yours, resold as a
metered API — in three milestones. Items are verified against the code, not
against earlier notes.

## Shipped in v0.4.0 (hardening)

Security: cleared environment and explicit tool set for every `claude -p`
child (one launcher), per-agent RBAC and account scoping on every route,
HMAC-signed webhooks, DNS pinning + timeouts on outbound calls, opaque 5xx,
tool credentials by connector reference, imported agents no longer default to
full autonomy. Billing: demo output and self-invokes are never billed, usage
is recorded with the real model and priced per model, stuck synchronous jobs
are failed, `call_agent` requires a full-auto target. Memory: step storage is
opt-in, recall is injected once, inner life is opt-in, run summaries decay
first. Reliability: one tool dispatcher, `web_search` only on providers with
live search, SSE lag is signalled. Ops: builds on macOS, CI (fmt / clippy
`-D warnings` / tests / release artifact / frontend check), systemd unit,
cached Docker layers, no baked-in admin password, `MASTER_KEY` required
outside demo mode.

## Shipped in v0.5.0 (sellable, part 1)

- **Memory fork per consumer** (`MemoryScope`, `MemoryMode`): consumer runs
  recall the publisher's memory read-only plus their own fork and write only
  to the fork; `GET/DELETE /api/v1/agents/:id/memory` for the consumer.
- **Prepaid credit**: `account_credit` / `credit_ledger` / `credit_hold`;
  worst-case reservation before the run (402 without credit), one-transaction
  settlement, stale-hold sweep, per-key rate limit (429), per-step output
  budget for consumer runs, optional price floor at publish, `GET /api/credit`,
  admin `POST /api/accounts/:id/credit`. Stripe is deliberately out: the
  ledger is the source of truth and a provider will only write `topup` rows.

## Shipped in v0.6.0 (sellable, part 2)

- **Memory provenance**: source, data subject, legal basis, retention and the
  ICM id on every row; consumer-run rows default to the consumer as subject
  under `contract`, user-authored ones to the author under `consent`.
- **Targeted erasure**: by subject across every scope, by single memory, and
  automatically past `retain_until` (maintenance loop). ICM and mirror.
- **Webhook rate limit** per event name (`WEBHOOK_RATE_LIMIT_PER_MIN`), counted
  before signature verification.
- Verified against icm 0.10.63: `icm consolidate --topic` is an exact match
  (only `recall --topic` is a prefix match), so no sibling folding.

## Milestone 1 — sellable

What a first paying customer would hit in the first hour.

- **Output-token cap on the claude-cli transport.** `INVOKE_MAX_OUTPUT_TOKENS`
  is honoured by OpenAI-compatible providers only; `claude -p` has no such
  flag, so the credit reservation is an estimate and a run can overshoot it
  (charged exactly, balance goes negative, next call blocked). Options: cap by
  killing the child once its streamed output exceeds the budget, or route
  consumer runs to a provider that honours `max_tokens`.

- **Payment provider adapter** writing `topup` rows (Stripe checkout →
  webhook → `billing::topup`), and a consumer-facing way to buy credit.
- **Real consumer accounts.** All users are created in the single default
  account, so publisher == consumer for every key today (self-invokes are now
  free, which makes the ledger honest but empty). Consumer sign-up creates its
  own account; publisher earnings and consumer spend then separate naturally.
- **Per-user rate limit** on the plan-spending one-shots any member may call
  (`/api/agents/scaffold`, video analysis with an agent).

## Milestone 2 — credible

What makes "it knows me" true rather than a pitch.

- **Embeddings on.** Every ICM call passes `--no-embeddings`; recall is
  lexical and `content_keywords` takes the first eight tokens. Enable
  embeddings for store + recall and measure recall quality before/after.
- **Blend recall**: query-scoped hits + top-weight high-importance items,
  de-duplicated, truncated on entry boundaries instead of `chars().take()`.
- **Per-agent maintenance**: `decay`/`prune` run on the whole ICM DB on one
  interval and race live runs on the same topic; serialise with a per-agent
  lock and exclude `high`/`critical` items from consolidation merges.
- **Daily ingestion pipeline**: connectors → normalise → de-duplicate → store
  with provenance. The plumbing exists (`connectors.rs`, `mcp.rs`,
  `scheduler.rs`); the pipeline does not. No de-duplication at `store` today.
- **Automatic persona evolution**: `evolve-persona` is manual; the
  `persona_evolution` toggle is read but drives nothing. Run it from the
  consolidation pass (factor the logic out of the HTTP handler).
- **Bench harness** (`icm bench-agent` / `bench-recall`): prove
  ICM-personalised vs vanilla and catch recall regressions in CI.
- **Consolidation summariser** uses the shared `claude` plan credential; route
  it through the account's provider like everything else.

## Milestone 3 — sustainable

Cost, performance and debt.

- Stop re-injecting the full 4000-char memory block plus the growing session
  into every step (prompt tokens grow step over step).
- `[profile.release]` is set; the VM still runs `cargo watch` — deploy the CI
  binary with `deploy/systemd/takoia.service` instead of building on the host.
- Frontend: drop the 2.5s `getJob` poll that duplicates SSE; narrow the canvas
  `$effect`; a Chat page (conversational front door to a persistent agent).
- `event_log` `message LIKE '%…%'` is a full scan: composite index or FTS.
- Dedupe the remaining `Command::new("icm")` builders (memory.rs) and the
  `strftime` literals behind small helpers.
- Backup / restore (agents + ICM DB + mirror), "duplicate agent with or
  without memory" (the most direct demo of the moat), builder palette audit,
  hide `claude -p` from user-facing video-analysis messages, GitHub repo
  metadata.
- Landlock `share_tmp=false` by default once TMPDIR is confirmed honoured on
  the target hosts.
