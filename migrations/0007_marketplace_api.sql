-- Marketplace: token-based pricing, consumer API keys, and per-invoke usage.

-- Price charged to consumers per 1k OUTGOING (completion) tokens.
ALTER TABLE agents ADD COLUMN price_per_1k_output_tokens REAL NOT NULL DEFAULT 0;
-- Share of revenue paid to the publisher (0..1); the platform keeps the rest.
ALTER TABLE agents ADD COLUMN revenue_share REAL NOT NULL DEFAULT 0.7;

-- Consumer API keys: one account can hold several keys to call public agents.
CREATE TABLE api_keys (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name        TEXT NOT NULL DEFAULT '',
    -- Only a hash is stored; the plaintext is shown once at creation.
    key_hash    TEXT NOT NULL UNIQUE,
    key_prefix  TEXT NOT NULL DEFAULT '',
    revoked     INTEGER NOT NULL DEFAULT 0,
    last_used_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX idx_api_keys_account ON api_keys(account_id);

-- One row per marketplace invoke: who called which published agent, the token
-- usage, the amount billed to the consumer and the share owed to the publisher.
CREATE TABLE marketplace_usage (
    id                TEXT PRIMARY KEY,
    agent_id          TEXT NOT NULL,
    publisher_account TEXT NOT NULL,
    consumer_account  TEXT NOT NULL,
    job_id            TEXT,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    billed_usd        REAL NOT NULL DEFAULT 0,
    publisher_usd     REAL NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX idx_mkt_usage_publisher ON marketplace_usage(publisher_account);
CREATE INDEX idx_mkt_usage_consumer ON marketplace_usage(consumer_account);
