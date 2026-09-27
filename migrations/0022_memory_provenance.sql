-- Provenance on every memory row: where it came from, whose data it is, on
-- what legal basis it is kept, until when, and the ICM id so a single memory
-- can be erased on both sides. Required before selling an agent trained on
-- personal context (targeted erasure, data lineage).
ALTER TABLE memories ADD COLUMN source TEXT NOT NULL DEFAULT 'run';
ALTER TABLE memories ADD COLUMN subject TEXT;
ALTER TABLE memories ADD COLUMN legal_basis TEXT;
ALTER TABLE memories ADD COLUMN retain_until TEXT;
ALTER TABLE memories ADD COLUMN job_id TEXT;
ALTER TABLE memories ADD COLUMN icm_id TEXT;
-- Existing consumer-fork rows are that consumer's data under the marketplace
-- contract; without this backfill targeted erasure would miss them. (Their
-- ICM ids are unknown: erasing such a fork falls back to the fork topic.)
UPDATE memories SET subject = consumer_account, legal_basis = 'contract'
 WHERE consumer_account IS NOT NULL AND subject IS NULL;
CREATE INDEX idx_memories_subject ON memories(agent_id, subject);
CREATE INDEX idx_memories_retain ON memories(retain_until);
