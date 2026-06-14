-- Performance indexes (additive DDL only, no data changes).
-- Speeds up the worker claim loop, per-agent revenue rollups, and job log queries.

-- Worker claim_next: filter status='queued' ORDER BY created_at.
CREATE INDEX IF NOT EXISTS idx_jobs_claim ON jobs(status, created_at);

-- Per-agent revenue rollups over marketplace usage.
CREATE INDEX IF NOT EXISTS idx_mkt_usage_agent ON marketplace_usage(agent_id);

-- Logs query: filter by job_id and order by created_at.
CREATE INDEX IF NOT EXISTS idx_event_log_job_created ON event_log(job_id, created_at);
