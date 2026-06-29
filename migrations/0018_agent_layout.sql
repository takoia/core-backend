-- Persist the builder graph layout (node positions) so the schema keeps every
-- box exactly where the user placed it across sessions and navigation.
-- JSON object: { "<block_key>": { "x": <number>, "y": <number> }, ... }
ALTER TABLE agents ADD COLUMN layout TEXT NOT NULL DEFAULT '{}';
