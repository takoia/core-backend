-- Marketplace & monetization: an owner builds an expert agent, personalizes it
-- over time, then publishes it (public/private) and monetizes it per run.

ALTER TABLE agents ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';   -- private | public
ALTER TABLE agents ADD COLUMN expertise_domain TEXT NOT NULL DEFAULT '';     -- e.g. "trading"
ALTER TABLE agents ADD COLUMN price_per_run_usd REAL NOT NULL DEFAULT 0;     -- charged to consumers per run
ALTER TABLE agents ADD COLUMN published_at TEXT;                             -- when made public
ALTER TABLE agents ADD COLUMN runs_count INTEGER NOT NULL DEFAULT 0;         -- popularity signal

CREATE INDEX idx_agents_visibility ON agents(visibility);
