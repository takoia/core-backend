-- Recurring autonomous runs. A schedule re-creates an objective + job for a
-- full-auto agent on an interval, so the agent loops and accumulates ICM memory
-- (learns over time). Supports a cron expression or a simple interval_seconds
-- (handy for fast local tests).

ALTER TABLE schedules ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE schedules ADD COLUMN prompt TEXT NOT NULL DEFAULT '';
ALTER TABLE schedules ADD COLUMN interval_seconds INTEGER;   -- if set, overrides cron for fast loops
ALTER TABLE schedules ADD COLUMN run_count INTEGER NOT NULL DEFAULT 0;
