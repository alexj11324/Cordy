CREATE TABLE channel_installation_runtime_observation (
    installation_id uuid NOT NULL,
    state text NOT NULL CHECK (state IN ('starting', 'healthy', 'degraded', 'offline', 'error')),
    observed_at timestamptz NOT NULL,
    error_code text,
    error_summary text,
    observer_token text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
