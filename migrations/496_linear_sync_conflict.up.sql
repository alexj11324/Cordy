-- One row per conflicting shared field. Values are kept as JSON so the
-- conflict center can render and resolve future provider-compatible values
-- without losing the exact base/local/remote evidence.
-- Relationships are enforced by application transactions, not database FKs.
CREATE TABLE linear_sync_conflict (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    binding_id UUID NOT NULL,
    link_id UUID NOT NULL,
    patchbay_issue_id UUID NOT NULL,
    linear_issue_id TEXT NOT NULL,
    field TEXT NOT NULL,
    base_value JSONB NOT NULL,
    local_value JSONB NOT NULL,
    remote_value JSONB NOT NULL,
    source_event_id TEXT NOT NULL,
    source_event_at_ms BIGINT,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'resolved', 'dismissed')),
    resolution TEXT
        CHECK (resolution IS NULL OR resolution IN ('local', 'remote', 'manual')),
    resolved_value JSONB,
    resolved_by_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
