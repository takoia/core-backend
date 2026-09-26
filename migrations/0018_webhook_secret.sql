-- Inbound webhooks must be signed. Each agent gets its own HMAC secret; the
-- sender puts `sha256=<hex hmac-sha256(secret, raw body)>` in
-- `X-Takoia-Signature`. Existing agents receive a random secret now so the
-- public /api/webhooks/:event route stops accepting unsigned payloads.
ALTER TABLE agents ADD COLUMN webhook_secret TEXT;
UPDATE agents SET webhook_secret = lower(hex(randomblob(24))) WHERE webhook_secret IS NULL;
