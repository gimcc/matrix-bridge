-- Add exclude_sources column to webhooks.
-- Comma-separated list of platform IDs whose messages should NOT be
-- forwarded to this webhook. Empty string means no exclusions.
ALTER TABLE webhooks ADD COLUMN exclude_sources TEXT NOT NULL DEFAULT '';
