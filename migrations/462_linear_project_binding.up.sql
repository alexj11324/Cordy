-- Project-level Linear mapping. Relationships are enforced by application
-- transactions so workspace/project deletion can remain explicit and auditable.
CREATE TABLE linear_project_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    cordy_project_id UUID NOT NULL,
    linear_project_id TEXT NOT NULL,
    linear_team_id TEXT,
    status TEXT NOT NULL DEFAULT 'draft'
        CHECK (status IN ('draft', 'active', 'paused', 'tombstone')),
    sync_mode TEXT NOT NULL DEFAULT 'not_synced'
        CHECK (sync_mode IN ('import', 'publish', 'two_way', 'not_synced')),
    initial_source_of_truth TEXT
        CHECK (initial_source_of_truth IS NULL OR initial_source_of_truth IN ('linear', 'cordy')),
    status_mapping JSONB NOT NULL DEFAULT '{}'::jsonb,
    agent_label_mapping JSONB NOT NULL DEFAULT '{}'::jsonb,
    activated_at TIMESTAMPTZ,
    paused_at TIMESTAMPTZ,
    created_by_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
