-- Per-agent persona: a seed identity/voice injected into every step's system
-- prompt. It is the static half of personalization; the ICM memory (recalled +
-- consolidated) is the evolving half, so the effective identity grows over time.
ALTER TABLE agents ADD COLUMN persona TEXT NOT NULL DEFAULT '';
