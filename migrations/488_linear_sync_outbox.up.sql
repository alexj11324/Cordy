-- Durable outbound Linear mutations. The row is written in the same
-- transaction as the canonical Issue mutation; the worker owns provider I/O.
-- Relationships are enforced by application transactions, not database FKs.
CREATE TABLE linear_sync_outbox (
    id UUID NOT NULL PRIMARY KEY,
    workspace_id UUID NOT NULL,
    binding_id UUID NOT NULL,
    issue_id UUID NOT NULL,
    event_key TEXT NOT NULL,
    event_type TEXT NOT NULL CHECK (event_type IN ('issue_created', 'issue_updated')),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_by TEXT,
    locked_until TIMESTAMPTZ,
    max_attempts INTEGER NOT NULL DEFAULT 8 CHECK (max_attempts > 0),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    processed_at TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
