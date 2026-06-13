-- TakoIA initial schema (SQLite dialect).
-- UUIDs are stored as TEXT, timestamps as ISO-8601 TEXT, JSON as TEXT,
-- encrypted secrets as BLOB. Designed to port to Postgres with minimal change.

PRAGMA foreign_keys = ON;

-- ── Accounts ────────────────────────────────────────────────────────────────
CREATE TABLE accounts (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- ── Agents ──────────────────────────────────────────────────────────────────
CREATE TABLE agents (
    id              TEXT PRIMARY KEY,
    account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    -- full_auto | confirm_before_action
    autonomy_level  TEXT NOT NULL DEFAULT 'confirm_before_action',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_agents_account ON agents(account_id);

-- ── Agent step configs: one row per step of the 4-step loop ──────────────────
CREATE TABLE agent_step_configs (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    -- analyse | decision | action | restitution
    step_type       TEXT NOT NULL,
    system_prompt   TEXT NOT NULL DEFAULT '',
    -- JSON: { provider, model, allowed_tools: [..], temperature, ... }
    options         TEXT NOT NULL DEFAULT '{}',
    position        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (agent_id, step_type)
);
CREATE INDEX idx_step_configs_agent ON agent_step_configs(agent_id);

-- ── Connectors: encrypted credentials per account (LLM providers, Discord…) ──
CREATE TABLE connectors (
    id                TEXT PRIMARY KEY,
    account_id        TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- llm | discord
    kind              TEXT NOT NULL,
    name              TEXT NOT NULL,
    base_url          TEXT NOT NULL DEFAULT '',
    model             TEXT NOT NULL DEFAULT '',
    -- chacha20poly1305: nonce(12) || ciphertext; NULL when no secret needed
    encrypted_secret  BLOB,
    meta              TEXT NOT NULL DEFAULT '{}',
    is_default        INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (account_id, kind, name)
);
CREATE INDEX idx_connectors_account ON connectors(account_id);

-- ── Objectives: a user goal that spawns a job ────────────────────────────────
CREATE TABLE objectives (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    prompt      TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_objectives_agent ON objectives(agent_id);

-- ── Jobs: the queue (queued | running | awaiting_approval | done | failed) ───
CREATE TABLE jobs (
    id            TEXT PRIMARY KEY,
    objective_id  TEXT NOT NULL REFERENCES objectives(id) ON DELETE CASCADE,
    agent_id      TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status        TEXT NOT NULL DEFAULT 'queued',
    error         TEXT,
    -- worker lease: who picked it and when (for crash recovery)
    locked_at     TEXT,
    started_at    TEXT,
    finished_at   TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_jobs_status ON jobs(status);
CREATE INDEX idx_jobs_agent ON jobs(agent_id);

-- ── Steps: one row per executed step of a job ────────────────────────────────
CREATE TABLE steps (
    id          TEXT PRIMARY KEY,
    job_id      TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    -- analyse | decision | action | restitution
    step_type   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending', -- pending | running | done | failed
    input       TEXT NOT NULL DEFAULT '{}',
    output      TEXT NOT NULL DEFAULT '{}',
    position    INTEGER NOT NULL DEFAULT 0,
    started_at  TEXT,
    finished_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_steps_job ON steps(job_id);

-- ── Approvals: human-in-the-loop gate for sensitive actions ──────────────────
CREATE TABLE approvals (
    id          TEXT PRIMARY KEY,
    job_id      TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    step_id     TEXT REFERENCES steps(id) ON DELETE CASCADE,
    -- pending | approved | rejected
    status      TEXT NOT NULL DEFAULT 'pending',
    summary     TEXT NOT NULL DEFAULT '',
    -- JSON description of the action awaiting approval
    payload     TEXT NOT NULL DEFAULT '{}',
    decided_at  TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_approvals_job ON approvals(job_id);
CREATE INDEX idx_approvals_status ON approvals(status);

-- ── Memories: permanent agent memory (read at Analyse, written at Restitution)─
CREATE TABLE memories (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_memories_agent ON memories(agent_id);

-- ── Token usage: basis for usage-based billing ───────────────────────────────
CREATE TABLE token_usage (
    id                 TEXT PRIMARY KEY,
    account_id         TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    agent_id           TEXT REFERENCES agents(id) ON DELETE SET NULL,
    job_id             TEXT REFERENCES jobs(id) ON DELETE SET NULL,
    provider           TEXT NOT NULL,
    model              TEXT NOT NULL DEFAULT '',
    prompt_tokens      INTEGER NOT NULL DEFAULT 0,
    completion_tokens  INTEGER NOT NULL DEFAULT 0,
    -- notional cost estimate (USD); real cost is the flat plan
    estimated_cost     REAL NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_token_usage_account ON token_usage(account_id);
CREATE INDEX idx_token_usage_job ON token_usage(job_id);

-- ── Schedules: cron triggers for recurring agents ────────────────────────────
CREATE TABLE schedules (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    objective_id  TEXT REFERENCES objectives(id) ON DELETE SET NULL,
    cron_expr     TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    last_run_at   TEXT,
    next_run_at   TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_schedules_agent ON schedules(agent_id);
