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
- Verified against icm 0.10.63: `icm consolidate --topic` is an exact match,
  so no sibling folding. (`recall --topic` is a substring match either way
  round, not a prefix match as first noted — see v0.7.0.)

## Shipped in v0.7.0 (permanent memory, lot 1)

An agent trained for months keeps what it learnt, and what a buyer gets is the
distilled expertise, not the trainer's history.

- **Nothing is summarised away any more.** The maintenance loop used to run
  `icm consolidate` as soon as an agent had six rows, then `icm decay` and
  `icm prune` on the whole ICM database every five minutes: consolidation
  deleted every non-critical original — `high` importance included — and an
  unrecalled memory was pruned within hours. All three are gone from the loop;
  it now only erases rows past their `retain_until`, then distils. ICM's own
  decay (at most once a day, at recall) still reorders, and deletes nothing.
- **Two layers** (migration `0023`): *episodes* (owner topic and consumer
  forks, verbatim, kept) and *knowledge* (`takoia/know/<agent>`, distilled).
  `memory_derivations` links every knowledge row to the episodes behind it.
- **Distillation** (`src/distill.rs`): six pending owner episodes — or one that
  has waited a day — trigger one LLM call on the account's own default
  provider (no tools, metered in `token_usage`, canned demo provider refused,
  180 s cap, back-off on failure). Strict JSON in, validated (kinds, length,
  one line, no e-mail address, no data subject of the episodes, retire ids
  among those shown and at most two more than the items returned); an unusable
  answer changes nothing. Consumer forks and the agent's own reflections are
  not distilled (an idle agent buys no model call). The agent's lock is
  held to read the snapshot and to write, not while the model answers: an
  answer whose snapshot was erased or expired in between is discarded.
- **Honest erasure**: erasing an episode — by subject, by id, by retention, by
  scope — erases the knowledge derived from it and sends the surviving
  episodes back to be distilled without it. An ICM entry shared by several
  mirror rows holding the same content is forgotten only with the last of
  them; one that holds another row's text (ICM merges near-duplicates when
  embeddings are on) is forgotten and the surviving rows stored again.
  Deleting an agent forgets its ICM topics, twice: before and after its row
  goes, so a run in flight leaves nothing. A per-agent lock serialises
  distillation and erasure, and the retention sweep runs on its own task.
- **De-duplication at store**: same content (case and spacing aside) for the
  same scope, layer and subject is stored once. ICM itself already folded an
  exact (topic, content) repeat into the existing entry; what was missing was
  the mirror, which grew a row per repeat. A repeat can bring the memory's
  `retain_until` forward, never extend it, and gives a row that missed ICM its
  copy; the mirror row is written before ICM, so a store that fails (agent
  deleted) or loses its row takes nothing to ICM or takes it back.
- **Recall** in one shape (`- [importance] text`): knowledge first, then the
  owner's episodes — or, for a marketplace consumer, their own fork. A
  consumer run no longer reads the publisher's episodes, nor the publisher's
  raw corrections. Query hits are filtered to the exact topic (asking ICM for
  four times the limit, since it cuts before the filter), then top-weight
  entries, then the mirror; budgets are spent on whole entries.
- One builder for every `icm` command; `MEMORY_EMBEDDINGS=true` opts into
  embeddings (experimental: 2-20 s per call, the model is loaded per process).
- `cargo clippy -D warnings` is green again on current toolchains.

Known limits of this lot: the over-fetch only mitigates sibling topics (an
agent whose id is a substring of another's, with more than three times the
limit of better-ranked matches there, falls back to its top-weight entries);
recall is still lexical by default; knowledge is recalled by weight, not by
relevance to the task, so past its 2000-char budget the lowest-weight rows are
never shown; a consumer gets nothing of the publisher's memory until a first
distillation has run (minutes with six episodes, a day with fewer, never while
the account has no working provider); an erasure waits for a
distillation only while it writes (a dozen `icm` calls, seconds with
embeddings on); knowledge that builds on an earlier row without replacing it
is tied to that row's episodes only if the model says so (`based_on`); an
answer refused for retiring too much leaves its episodes pending, so an
episode that steers the model stalls that agent's distillation (visible in the
logs, spaced out by the back-off) until it is erased.

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

What makes "it knows me" true rather than a pitch. Done in v0.7.0 and removed
from this list: blended recall on entry boundaries, the per-agent lock, the end
of destructive consolidation (and with it the summariser that used the shared
`claude` credential — distillation goes through the account's provider),
de-duplication at `store`.

- **Embeddings by default, through a warm ICM daemon.** The flag exists
  (`MEMORY_EMBEDDINGS`) but every `icm` call is a new process that loads the
  model: 2-20 s against ~30 ms, several times per run. Keep one ICM process
  warm (daemon or MCP server), then switch the default and measure recall
  quality before/after. Until then recall is lexical and `content_keywords`
  takes the first eight tokens. With embeddings a near-duplicate is folded
  into an existing ICM entry, so two different mirror rows share one ICM id:
  erasure handles it (forget, then store the survivors again), at the price of
  more model loads — one more reason for the daemon.
- **Automatic persona evolution**: `evolve-persona` is manual; the
  `persona_evolution` toggle is read but drives nothing. Run it after a
  distillation pass, from the knowledge layer (factor the logic out of the
  HTTP handler).
- **Daily ingestion pipeline**: connectors → normalise → store with
  provenance. The plumbing exists (`connectors.rs`, `mcp.rs`, `scheduler.rs`)
  and `store` now de-duplicates; the pipeline does not exist.
- **Bench harness** (`icm bench-agent` / `bench-recall`): prove
  ICM-personalised vs vanilla, distilled vs raw, and catch recall regressions
  in CI (which has no `icm` today: the ICM side of the tests is skipped there).
- **Distillation and recall, next steps**: pick the knowledge shown to a run
  by relevance to its objective, not by weight alone; consumer forks are not
  distilled; a
  knowledge-only purge is not rebuilt (its episodes stay marked as distilled);
  tie a knowledge row to the rows it builds on without relying on the model
  (`based_on` is declared, not checked).
- **ICM feedback rows are not erasable.** A correction is written twice: as
  an owner episode (erasable, distilled) and with `icm feedback record`. icm
  0.10.65 has no command to delete a feedback entry and `forget --topic`
  leaves them, so they outlive a purge and the agent's deletion and are still
  injected into owner runs. Either get a delete into ICM or serve corrections
  from the mirror and stop writing them there.

## Milestone 3 — sustainable

Cost, performance and debt.

- Stop re-injecting the full recalled memory (two blocks of up to 2000 chars)
  plus the growing session into every step (prompt tokens grow step over step).
- `[profile.release]` is set; the VM still runs `cargo watch` — deploy the CI
  binary with `deploy/systemd/takoia.service` instead of building on the host.
- Frontend: drop the 2.5s `getJob` poll that duplicates SSE; narrow the canvas
  `$effect`; a Chat page (conversational front door to a persistent agent).
- `event_log` `message LIKE '%…%'` is a full scan: composite index or FTS.
- Put the `strftime` literals behind a small helper.
- Backup / restore (agents + ICM DB + mirror), "duplicate agent with or
  without memory" (the most direct demo of the moat), builder palette audit,
  hide `claude -p` from user-facing video-analysis messages, GitHub repo
  metadata.
- Landlock `share_tmp=false` by default once TMPDIR is confirmed honoured on
  the target hosts.
