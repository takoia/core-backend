-- ICM topics that hold copies an erasure could not remove: `icm` was missing
-- or failing when the rows were erased from this database. The mirror no
-- longer says those copies exist, so they are remembered here: a later purge
-- of the topic on a host without `icm` is not reported complete, and
-- maintenance wipes the topic again once ICM answers and its scope is empty.
-- No foreign key: a topic outlives the agent whose deletion left it behind.
CREATE TABLE icm_residue (
    topic      TEXT PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
