-- Linear installation foundation. External integration relationships are
-- enforced by application transactions.
CREATE TABLE linear_connection (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    organization_id TEXT NOT NULL,
    organization_name TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    access_token_encrypted TEXT NOT NULL,
    refresh_token_encrypted TEXT NOT NULL,
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
    code_verifier_encrypted TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE linear_sync_inbox (
    id UUID NOT NULL,
    connection_id UUID NOT NULL,
    delivery_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
