-- Pluggable execution sandbox for agent `claude -p` runs. Singleton row (id = 1).
-- `params` is plain JSON config (allow_network, mem_mb, cpus, image, launcher,
-- extra_args) — no secrets. Default backend = none (host execution, as before).
CREATE TABLE sandbox_backend (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    kind       TEXT NOT NULL DEFAULT 'landlock',
    params     TEXT,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
-- Default to the native Landlock filesystem sandbox (no external binary needed).
INSERT INTO sandbox_backend (id, kind) VALUES (1, 'landlock');
