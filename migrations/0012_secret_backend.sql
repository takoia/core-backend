-- Pluggable external secret storage for connector / AI-tool secrets.
-- Singleton row (id = 1). `params` holds the cipher-encrypted JSON config of the
-- selected backend (e.g. vault name, project, region). Default backend = local.
CREATE TABLE secret_backend (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    kind       TEXT NOT NULL DEFAULT 'local',
    params     BLOB,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
INSERT INTO secret_backend (id, kind) VALUES (1, 'local');
