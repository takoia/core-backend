-- Memory layers. An agent's memory is no longer one pile that maintenance
-- summarises in place: raw EPISODES are kept as they were learnt, and a
-- separate KNOWLEDGE layer holds what distillation extracts from them (the
-- transferable expertise a marketplace buyer gets, without the trainer's
-- private history). Existing rows — including the old key='consolidated'
-- ones — stay episodes.
ALTER TABLE memories ADD COLUMN layer TEXT NOT NULL DEFAULT 'episode';
-- SHA-256 of the normalised content: the de-duplication key at store time.
-- NULL on rows written before this migration (they are never deduplicated).
ALTER TABLE memories ADD COLUMN content_hash TEXT;
-- When an episode was last folded into the knowledge layer; NULL = pending.
ALTER TABLE memories ADD COLUMN distilled_at TEXT;

-- Which episodes a knowledge row was distilled from, so erasing an episode can
-- take the knowledge derived from it along.
CREATE TABLE memory_derivations (
    knowledge_id TEXT NOT NULL,
    episode_id   TEXT NOT NULL,
    PRIMARY KEY (knowledge_id, episode_id)
);
CREATE INDEX idx_memory_derivations_episode ON memory_derivations(episode_id);

-- The same content is stored once per (agent, scope, layer, subject): never
-- de-duplicated across scopes or data subjects.
CREATE UNIQUE INDEX idx_memories_content_hash
    ON memories(agent_id, COALESCE(consumer_account, ''), layer, COALESCE(subject, ''), content_hash)
    WHERE content_hash IS NOT NULL;
-- Several mirror rows may point at one ICM entry (same content, different
-- subject): erasure looks the id up before forgetting it there.
CREATE INDEX idx_memories_icm ON memories(icm_id);
