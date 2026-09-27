-- Prepaid credit for marketplace consumers. `marketplace_usage` stays the
-- record of what each invoke cost; these tables make it enforceable: a call
-- is admitted only if the account's available credit (balance minus holds in
-- flight) covers its worst case, and settled in the same transaction that
-- records the usage. The ledger is the source of truth for the balance; a
-- payment provider, when one is wired, only writes `topup` rows.
CREATE TABLE account_credit (
    account_id     TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    balance_usd    REAL NOT NULL DEFAULT 0,
    -- Ceiling on what a single invoke may reserve, whatever the price.
    max_invoke_usd REAL NOT NULL DEFAULT 5.0,
    updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE credit_ledger (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL,
    api_key_id  TEXT,
    delta_usd   REAL NOT NULL,
    -- topup | invoke | refund | adjustment
    reason      TEXT NOT NULL,
    job_id      TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_credit_ledger_account ON credit_ledger(account_id, created_at);
CREATE INDEX idx_credit_ledger_key ON credit_ledger(api_key_id, created_at);

-- A reservation while an invoke runs; deleted when it settles or fails.
CREATE TABLE credit_hold (
    id          TEXT PRIMARY KEY,
    account_id  TEXT NOT NULL,
    api_key_id  TEXT,
    amount_usd  REAL NOT NULL,
    job_id      TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_credit_hold_account ON credit_hold(account_id);
CREATE INDEX idx_credit_hold_key ON credit_hold(api_key_id, created_at);

-- Per-key request rate (holds + ledger rows in the last minute).
ALTER TABLE api_keys ADD COLUMN rate_limit_per_min INTEGER NOT NULL DEFAULT 60;
