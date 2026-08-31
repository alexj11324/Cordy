-- Provider-neutral identity between one Patchbay issue and one Linear issue.
-- Relationships are enforced by application transactions; no database FKs.
CREATE TABLE linear_issue_link (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    binding_id UUID NOT NULL,
    patchbay_issue_id UUID NOT NULL,
    linear_issue_id TEXT NOT NULL,
    linear_identifier TEXT NOT NULL,
    last_common_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    remote_updated_at TIMESTAMPTZ,
    last_remote_event_at_ms BIGINT,
    last_remote_event_id TEXT,
    sync_status TEXT NOT NULL DEFAULT 'active'
        CHECK (sync_status IN ('active', 'paused', 'conflict', 'deleted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
