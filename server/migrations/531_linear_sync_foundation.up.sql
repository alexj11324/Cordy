-- Linear OAuth, project mapping, and durable sync state. All relationships
-- are validated and cleaned up by application transactions; no database FKs.
CREATE TABLE linear_connection (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    organization_id TEXT NOT NULL,
    organization_name TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    access_token_encrypted BYTEA NOT NULL,
    refresh_token_encrypted BYTEA NOT NULL,
    token_expires_at TIMESTAMPTZ NOT NULL,
    scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
    webhook_id TEXT,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'reauthorization_required', 'revoked')),
    last_success_at TIMESTAMPTZ,
    last_error TEXT,
    created_by_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_oauth_state (
    id UUID NOT NULL,
    state_hash TEXT NOT NULL,
    workspace_id UUID NOT NULL,
    user_id UUID NOT NULL,
    code_verifier_encrypted BYTEA NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_project_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    patchbay_project_id UUID NOT NULL,
    linear_project_id TEXT NOT NULL,
    linear_team_id TEXT,
    status TEXT NOT NULL DEFAULT 'draft'
        CHECK (status IN ('draft', 'active', 'paused', 'tombstone')),
    sync_mode TEXT NOT NULL DEFAULT 'not_synced'
        CHECK (sync_mode IN ('import', 'publish', 'two_way', 'not_synced')),
    initial_source_of_truth TEXT
        CHECK (initial_source_of_truth IS NULL OR initial_source_of_truth IN ('linear', 'patchbay')),
    status_mapping JSONB NOT NULL DEFAULT '{}'::jsonb,
    agent_label_mapping JSONB NOT NULL DEFAULT '{}'::jsonb,
    activated_at TIMESTAMPTZ,
    paused_at TIMESTAMPTZ,
    created_by_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

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

CREATE TABLE linear_sync_inbox (
    id UUID NOT NULL,
    connection_id UUID NOT NULL,
    delivery_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_by TEXT,
    locked_until TIMESTAMPTZ,
    max_attempts INTEGER NOT NULL DEFAULT 8 CHECK (max_attempts > 0),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    processed_at TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ
);

CREATE TABLE linear_sync_outbox (
    id UUID NOT NULL,
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

CREATE TABLE linear_member_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    patchbay_user_id UUID NOT NULL,
    linear_user_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

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
