-- Custom emoji/icon per agent (editable in the builder).
ALTER TABLE agents ADD COLUMN icon TEXT NOT NULL DEFAULT '';
