-- call_agent sub-runs are separate jobs; billing a marketplace invoke must
-- cover the whole chain, so each sub-job records its parent.
ALTER TABLE jobs ADD COLUMN parent_job_id TEXT;
CREATE INDEX idx_jobs_parent ON jobs(parent_job_id);
