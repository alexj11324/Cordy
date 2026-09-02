-- Managed Slack OAuth state (Go mainline slice 6, port of the Rust 601 block).
-- One row per in-flight hosted install authorization: the state token hash
-- binds the Slack callback to the workspace + installer that started it, and
-- expires after ten minutes. No foreign keys by repository rule; workspace
-- teardown sweeps these rows explicitly (workspace_delete.sql), because a
-- deleted workspace never runs another install to purge them opportunistically.
CREATE TABLE IF NOT EXISTS slack_oauth_state (
    state_hash bytea NOT NULL,
    workspace_id uuid NOT NULL,
    installer_user_id uuid NOT NULL,
    redirect_url text NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
