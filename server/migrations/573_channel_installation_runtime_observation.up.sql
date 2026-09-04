-- Per-installation runtime health observations for hosted messaging
-- (Go mainline slice 6, port of the Rust 603 block). One row per channel
-- installation, rewritten by whichever supervisor last observed it; readers
-- use it to report starting / healthy / degraded / offline / error without
-- probing live connections. No foreign keys by repository rule; lifecycle
-- follows channel_installation through the application-owned sweeps.
CREATE TABLE IF NOT EXISTS channel_installation_runtime_observation (
    installation_id uuid NOT NULL,
    state text NOT NULL CHECK (state IN ('starting', 'healthy', 'degraded', 'offline', 'error')),
    observed_at timestamptz NOT NULL,
    error_code text,
    error_summary text,
    observer_token text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
