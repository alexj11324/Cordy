CREATE TABLE slack_oauth_state (
    state_hash bytea NOT NULL,
    workspace_id uuid NOT NULL,
    installer_user_id uuid NOT NULL,
    redirect_url text NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
