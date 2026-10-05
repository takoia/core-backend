-- The account a marketplace invoke ran for, on the invoke's own job (the
-- publisher's account for a self-invoke; NULL for every job that is not an
-- invoke). Until now it could only be read from the invoke's billing rows —
-- its credit hold while it runs, its usage row afterwards — and an invoke that
-- ends abnormally (client gone, server restarted, hold swept) has neither:
-- nothing said any more whose run it was, and so whose memory a correction of
-- it belongs to.
ALTER TABLE jobs ADD COLUMN invoked_by TEXT;
