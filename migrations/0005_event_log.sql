-- Enterprise audit trail: persist every job event for full traceability.
CREATE TABLE event_log (
    id          TEXT PRIMARY KEY,
    job_id      TEXT NOT NULL,
    kind        TEXT NOT NULL,
    step_type   TEXT,
    status      TEXT,
    message     TEXT NOT NULL DEFAULT '',
    data        TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_event_log_job ON event_log(job_id);
CREATE INDEX idx_event_log_created ON event_log(created_at);
