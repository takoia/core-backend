-- Brute-force protection: auto-ban an IP after too many failed logins.
CREATE TABLE security_settings (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    auto_ban_enabled    INTEGER NOT NULL DEFAULT 1,
    max_failed_attempts INTEGER NOT NULL DEFAULT 5,
    window_secs         INTEGER NOT NULL DEFAULT 900,  -- count failures within this window
    ban_secs            INTEGER NOT NULL DEFAULT 900,  -- ban duration once tripped
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
INSERT INTO security_settings (id) VALUES (1);

CREATE TABLE login_attempts (
    ip           TEXT PRIMARY KEY,
    failed_count INTEGER NOT NULL DEFAULT 0,
    window_start TEXT,
    banned_until TEXT,
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_login_attempts_ban ON login_attempts(banned_until);
