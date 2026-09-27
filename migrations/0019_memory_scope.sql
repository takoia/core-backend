-- Memory fork per consumer. A marketplace consumer's runs recall the
-- publisher's curated memory (read-only) plus their own fork (read/write),
-- so the agent keeps learning from the person who uses it without polluting
-- the publisher's expertise. NULL = the owner's memory (unchanged rows).
ALTER TABLE memories ADD COLUMN consumer_account TEXT;
CREATE INDEX idx_memories_scope ON memories(agent_id, consumer_account);
