-- ICM entries an erasure could not forget: `icm` was missing or failing when
-- their rows were erased from this database. `icm_residue` remembers a whole
-- topic, which maintenance can only wipe once its scope is empty; an entry
-- left in a topic that stays in use (an agent's own memory, its knowledge)
-- was never removed, and recall kept serving it. Remembered by id, it is
-- forgotten on its own as soon as ICM answers, whatever its topic holds.
-- No foreign key: an entry outlives the agent whose erasure left it behind.
CREATE TABLE icm_residue_ids (
    icm_id     TEXT PRIMARY KEY,
    topic      TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_icm_residue_ids_topic ON icm_residue_ids(topic);
