-- Linear integration storage.  These tables deliberately do not declare
-- foreign keys: workspace teardown and binding tombstones are owned by the
-- application transaction, just like the other external integrations.
CREATE TABLE linear_connection (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    organization_id TEXT NOT NULL,
    organization_name TEXT,
    actor_id TEXT,
    access_token_encrypted TEXT NOT NULL,
    refresh_token_encrypted TEXT NOT NULL,
    token_expires_at TIMESTAMPTZ,
    scopes JSONB NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'reauthorization_required', 'revoked')),
    created_by_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_oauth_state (
    id UUID NOT NULL,
    state_hash TEXT NOT NULL,
    workspace_id UUID NOT NULL,
    user_id UUID NOT NULL,
    code_verifier_encrypted TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_project_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    patchbay_project_id UUID,
    linear_project_id TEXT NOT NULL,
    default_linear_team_id TEXT,
    sync_mode TEXT NOT NULL DEFAULT 'two_way' CHECK (sync_mode IN ('two_way', 'pull_only', 'push_only')),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'out_of_scope', 'tombstone')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_status_binding (
    id UUID NOT NULL,
    project_binding_id UUID NOT NULL,
    patchbay_status TEXT NOT NULL,
    linear_status_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_issue_link (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    issue_id UUID NOT NULL,
    linear_issue_id TEXT NOT NULL,
    linear_identifier TEXT,
    project_binding_id UUID NOT NULL,
    remote_updated_at TIMESTAMPTZ,
    remote_snapshot JSONB NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'out_of_scope', 'tombstone', 'conflict')),
    last_pulled_at TIMESTAMPTZ,
    last_pushed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_member_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    member_id UUID,
    linear_user_id TEXT NOT NULL,
    normalized_email TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'unbound', 'diagnostic')),
    diagnostic TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_agent_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    agent_id UUID NOT NULL,
    linear_label_group_id TEXT NOT NULL,
    linear_label_id TEXT NOT NULL,
    label_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_relation_link (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    from_issue_id UUID NOT NULL,
    to_issue_id UUID NOT NULL,
    linear_relation_id TEXT,
    relation_type TEXT NOT NULL CHECK (relation_type IN ('parent', 'blocks', 'blocked_by')),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'conflict', 'tombstone')),
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
    processed_at TIMESTAMPTZ,
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE TABLE linear_sync_outbox (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    issue_id UUID,
    correlation_id UUID NOT NULL,
    operation TEXT NOT NULL,
    payload JSONB NOT NULL,
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at TIMESTAMPTZ,
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE TABLE linear_sync_conflict (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    issue_id UUID,
    linear_issue_id TEXT,
    field TEXT NOT NULL,
    local_value JSONB,
    remote_value JSONB,
    local_revision BIGINT,
    remote_updated_at TIMESTAMPTZ,
    correlation_id UUID,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'ignored')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ
);
