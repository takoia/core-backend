-- Multi-user accounts within the org, bearer-token sessions, and per-agent RBAC.

-- Users belong to the single org account (DEFAULT_ACCOUNT_ID). `is_admin` is the
-- org administrator who can manage users.
CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    account_id    TEXT NOT NULL,
    email         TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL DEFAULT '',
    password_hash TEXT NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_users_account ON users(account_id);

-- Opaque bearer-token sessions returned at login and validated on each request.
CREATE TABLE IF NOT EXISTS user_sessions (
    token      TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    expires_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_user_sessions_user ON user_sessions(user_id);

-- Per-agent role: 'owner' (full control + manage access + delete),
-- 'editor' (view + edit + run), 'viewer' (view only).
CREATE TABLE IF NOT EXISTS agent_permissions (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    user_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role     TEXT NOT NULL,
    PRIMARY KEY (agent_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_agent_perm_user ON agent_permissions(user_id);
