-- "Inner life" of an agent: an evolving affective state and kept commitments,
-- driven by the periodic ticks so the agent feels alive over time.
CREATE TABLE agent_state (
    agent_id    TEXT PRIMARY KEY,
    mood        TEXT NOT NULL DEFAULT 'curious',
    energy      REAL NOT NULL DEFAULT 0.6,
    familiarity INTEGER NOT NULL DEFAULT 0,
    reflection  TEXT,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Open loops the agent promised to follow up on; due ones are picked up on a tick.
CREATE TABLE agent_commitments (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    description TEXT NOT NULL,
    due_at      TEXT,
    done        INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_commitments_due ON agent_commitments(agent_id, done, due_at);
