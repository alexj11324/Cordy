-- Optional per-automation model override and tool allowlist. Missing values
-- mean "use the executor agent's model / tools". No foreign keys.

ALTER TABLE automation
    ADD COLUMN IF NOT EXISTS model TEXT,
    ADD COLUMN IF NOT EXISTS tools JSONB NOT NULL DEFAULT '{}'::jsonb;
