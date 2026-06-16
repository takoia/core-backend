-- Per-agent personalization toggles + the Big Five personality vector. Lets an
-- owner enable/disable each "human" feature per agent. All default on; the
-- personality is neutral until tuned.
CREATE TABLE agent_personalization (
    agent_id          TEXT PRIMARY KEY,
    reflection        INTEGER NOT NULL DEFAULT 1,
    emotions          INTEGER NOT NULL DEFAULT 1,
    initiative        INTEGER NOT NULL DEFAULT 1,
    commitments       INTEGER NOT NULL DEFAULT 1,
    persona_evolution INTEGER NOT NULL DEFAULT 1,
    personality       INTEGER NOT NULL DEFAULT 1,
    big_five          TEXT, -- JSON {openness,conscientiousness,extraversion,agreeableness,neuroticism}
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Dynamic Plutchik emotion vector lives on the affective state.
ALTER TABLE agent_state ADD COLUMN emotions TEXT;
