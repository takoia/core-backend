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
  180 s cap), one per agent per pass, on its 40 oldest pending episodes and
  its 60 most recent knowledge rows. Strict JSON in, validated (kinds, length,
  one line, no e-mail address, no data subject of the episodes, retire ids
  among those shown and at most two more than the items returned); an unusable
  answer writes nothing. Consumer forks and the agent's own reflections are
  not distilled (an idle agent buys no model call). The agent's lock is
  held to read the snapshot and to write, not while the model answers: an
  answer whose snapshot was erased or expired in between is discarded.
- **Distilled at publication.** `POST /api/agents/:id/publish` with
  `visibility: public` distils whatever owner episodes are waiting, in a
  background task — whatever their number or age, otherwise an ordinary
  distillation (same lock, validation, metering, refusal of the canned
  provider) — up to five in a row, stopping when nothing waits or one fails.
  The request does not wait for the model; its answer carries
  `knowledge_rows` and `distillation_started`. One distillation per agent runs
  at a time in the process, so a publication and the loop never pay twice for
  the same episodes. A TOML import that takes a private agent public
  (`POST /api/agents/import`) distils the same way; re-importing a public
  agent does not.
- **Two kinds of failure.** A provider failure (none configured, transport,
  timeout) is retried further and further apart, up to 64 passes. An unusable
  answer is not waited out: the next pass asks about half as many episodes
  (40 → 20 → 10 → 5 → 2 → 1, oldest first), which lets the sound ones through;
  when the one episode left gets an unusable answer it is **set aside** —
  marked as distilled without producing knowledge, kept like every episode,
  with a `distillation-quarantine` journal entry (episode id and kind of
  refusal, never its text nor the model's). A success brings the full snapshot
  back. An episode that steers the model no longer stalls its agent. An
  episode that waits alone without having been narrowed down to is asked
  about twice first. Three set aside in a row and the model is the suspect:
  the agent is waited out, and that count is cleared by a success only — it
  is kept while the agent has nothing waiting, so small backlogs are not set
  aside one after the other. An answer that copies the format example's
  `"<id>"` into `retire` is not refused for it: the placeholder retires
  nothing.
- **Derivations are checked, not only declared.** A new knowledge row inherits
  the episodes behind every shown row it retires, declares in `based_on`, or
  resembles (half of their words of four letters or more in common): a rule
  the model rewrites without saying so is still erased with the episodes
  behind the original.
- **Subject guard that does not over-match.** A data subject is looked for in
  an item only if it looks like a name or an identifier (any character that is
  not a lowercase letter — a digit, an uppercase letter, a space, a letter of
  a script without case — or eight characters and more): a memory filed under
  `client` no longer costs the agent every rule that says "client", and a
  short name in Arabic, Hebrew or CJK is still looked for.
- **Honest erasure**: erasing an episode — by subject, by id, by retention, by
  scope — erases the knowledge derived from it and sends the surviving
  episodes back to be distilled without it. An ICM entry shared by several
  mirror rows holding the same content is forgotten only with the last of
  them; one that holds another row's text (ICM merges near-duplicates when
  embeddings are on) is forgotten and the surviving rows stored again.
  Deleting an agent forgets its ICM topics, twice: before and after its row
  goes, so a run in flight leaves nothing. A per-agent lock serialises
  distillation and erasure, and the retention sweep runs on its own task.
  Purging the knowledge topic alone (`takoia/know/<agent>`) sends the owner's
  episodes back to be distilled (`requeued` in the answer), set-aside ones
  included: the layer is rebuilt by the next passes. The memory page labels
  that topic and lists its knowledge rows only.
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
- **Knowledge shown by relevance.** The knowledge block starts with the rows
  the run's objective hits (same search and exact-topic filter as the personal
  block), then the top-weight rows fill it up to the same 24 entries and 2000
  characters, each row once. Without a hit it is the top-weight rows, as
  before. A run's query is a whole prompt, and in keyword mode ICM answers one
  with every row any of its words is found in — as a substring, in the topic
  and the keywords too, so `a` or `agent` hit everything (icm 0.10.65). ICM is
  therefore asked about the prompt's salient words only (the ones a memory's
  keywords are made of), and its hits are kept and ranked on how many of them
  their text mentions. This applies to the personal block as well. With
  `MEMORY_EMBEDDINGS` the query goes to ICM as it is.
- **Corrections are episodes, and nothing else.** `POST /api/jobs/:id/feedback`
  used to write a correction twice: as an owner episode and with
  `icm feedback record`. icm has no command to delete a feedback entry and
  `forget --topic` leaves them, so the second copy outlived a purge and the
  agent's deletion. TakoIA no longer writes nor reads ICM
  feedback rows. A correction is one high-importance `correction` episode,
  stored in the scope of the run it corrects with that run's provenance — the
  consumer's fork for a marketplace run (sub-runs included), never the
  publisher's episodes — and erased like any memory. The account an invoke ran
  for is kept on its job (`jobs.invoked_by`, migration `0025`): an invoke
  abandoned before settlement has neither hold nor usage row, and its
  correction would otherwise have gone, prompt included, to the publisher's
  episodes. When nothing says whose run it was the feedback is refused (409).
  A run is given the corrections of its own scope: those its objective hits,
  then the most recent, five at most within 2000 characters. They are left out
  of the memory block, so none is injected twice. Rows that older versions
  wrote to ICM's feedback store are simply no longer read; they stay in the
  ICM database, which cannot delete them.
- **Hosts without `icm`.** The binary is probed at startup (`icm --version`)
  and again every five maintenance passes — every pass while it is missing, so
  one failed probe on a host that has ICM costs one pass. While it is missing
  nothing is spawned: stores write the mirror only, recall reads it, the
  back-fill is skipped, the memory page lists the mirror's topics, and there
  is one warning at startup instead of one per call. Erasure is then judged on
  facts: a row that never had an ICM id has no copy, so its erasure is
  complete; a row that had one still counts as failed. A whole topic is
  complete only if no row of its scope ever had a copy: the topics a purge
  could not clean are kept in `icm_residue` (migration `0024`), so asking
  again does not turn an incomplete purge into a complete one, and the ICM
  entries a row-by-row erasure could not forget are kept by id in
  `icm_residue_ids` (migration `0026`). Once ICM answers again, the
  maintenance loop forgets those entries one by one, whatever their topic
  still holds (a row that shared one is stored again), wipes the topics whose
  scope is empty and back-fills the rest. `GET /api/memory/overview` carries
  `icm_available`.
- One builder for every `icm` command; `MEMORY_EMBEDDINGS=true` opts into
  embeddings (experimental: 2-20 s per call, the model is loaded per process).
- `cargo clippy -D warnings` is green again on current toolchains.

Known limits of this lot: the over-fetch only mitigates sibling topics (an
agent whose id is a substring of another's, with more than three times the
limit of better-ranked matches there, falls back to its top-weight entries);
recall is still lexical by default, and so is relevance: a knowledge row comes
first when it shares a word of four letters or more with the first eight
salient words of the objective, a row that says the same thing in other words
does not, and on a host without `icm` nothing is searched (the knowledge block
is the most recent rows); a consumer gets nothing of the publisher's memory
until the distillation started at publication has answered, and never while
the account has no working provider; a publication distils 200 episodes at
most, the loop does the rest 40 per pass; an erasure waits for a distillation
only while it writes (a dozen `icm` calls, seconds with embeddings on); the
resemblance that ties a rewritten rule to the original is lexical, so a
rewrite sharing less than half its words with it, and not declared by the
model, is not tied to it; a set-aside episode is tried again only when an
erasure or a knowledge purge sends it back, at most three are set aside in a
row for one agent (past that the model is the suspect and the agent is waited
out like a provider that is down), and the snapshot sizes and that count are
kept in memory, so a restart starts again from a full snapshot and may set
three more aside; a data subject written all in lowercase letters and under
eight of them is no longer looked for in the items, and a longer one that is
an ordinary word still is; corrections reach the Analyse step only (the memory
block every step gets no longer repeats them), and the ones an objective hits
are looked for among the scope's 50 best hits; an invoke abandoned before
settlement that predates `jobs.invoked_by` and whose run stored nothing in a
fork cannot be corrected (409), and the publisher's own abandoned invoke from
that time neither; a host whose `icm` was removed is noticed at the next probe
(up to five passes), and calls fail one by one until then; an ICM copy an
erasure could not forget is still served to runs until ICM answers a
maintenance pass (twenty-five entries per pass), and a topic a purge could not
wipe, in a scope that has been written to since, is only settled by a purge
ICM confirms; rows without an ICM id written before ids were recorded count as
having no copy on a host without `icm`.

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
de-duplication at `store`, distillation at publication, checked derivations,
set-aside episodes, the rebuilt knowledge purge, knowledge shown by relevance,
corrections served from the mirror instead of ICM's feedback store, hosts
without `icm`.

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
- **Distillation and recall, next steps**: consumer forks are not distilled;
  a semantic rather than lexical test for "this row was written from that
  one", and for "this row is relevant to that objective", comes with the
  embeddings above.

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
