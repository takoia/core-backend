-- Marketplace inline invoke jobs are created already `running` and executed
-- synchronously inside the HTTP handler (read-only memory, metered + billed
-- there). Flag them so crash recovery never requeues them onto the background
-- worker, which would re-run them with write-enabled memory (polluting the
-- publisher's curated memory) and without billing.
ALTER TABLE jobs ADD COLUMN synchronous INTEGER NOT NULL DEFAULT 0;
