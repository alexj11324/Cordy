-- Add crash-safe leasing and retry scheduling to the Linear Inbox.
-- The worker must own a row before it can complete or retry it.
ALTER TABLE linear_sync_inbox
    ADD COLUMN available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN locked_by TEXT,
    ADD COLUMN locked_until TIMESTAMPTZ,
    ADD COLUMN max_attempts INTEGER NOT NULL DEFAULT 8,
    ADD COLUMN dead_lettered_at TIMESTAMPTZ;
