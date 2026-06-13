-- Declarative agents (TOML) + event choreography.
-- An agent declares the event that triggers it (trigger_on) and the events it
-- emits at the end of a run (emit). Wiring two agents = matching emit -> trigger_on.

ALTER TABLE agents ADD COLUMN author TEXT NOT NULL DEFAULT '';        -- expert credit
ALTER TABLE agents ADD COLUMN version TEXT NOT NULL DEFAULT '0.1.0';
ALTER TABLE agents ADD COLUMN trigger_on TEXT;                         -- event name that triggers this agent
ALTER TABLE agents ADD COLUMN emit TEXT NOT NULL DEFAULT '[]';         -- JSON array of emitted event names
ALTER TABLE agents ADD COLUMN definition_toml TEXT NOT NULL DEFAULT ''; -- source-of-truth TOML

CREATE INDEX idx_agents_trigger ON agents(trigger_on);

-- Chain bookkeeping on jobs (event choreography + loop safety).
ALTER TABLE jobs ADD COLUMN chain_depth INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN trigger_event TEXT;                        -- event that spawned this job, if any
