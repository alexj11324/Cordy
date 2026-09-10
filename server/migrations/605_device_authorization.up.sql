-- OAuth 2.0 device authorization records used by the headless CLI login.
-- Codes are stored only as SHA-256 hashes; the raw values are returned once
-- to the requesting CLI and are never recoverable from the database.
-- User ownership and token issuance are enforced by the application in a
-- transaction. There are intentionally no foreign keys or cascades.
CREATE TABLE device_authorization (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    client_name TEXT NOT NULL,
    device_code_hash TEXT NOT NULL,
    user_code_hash TEXT NOT NULL,
    user_id UUID,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied', 'consumed', 'expired')),
    expires_at TIMESTAMPTZ NOT NULL,
    interval_seconds INTEGER NOT NULL DEFAULT 5
        CHECK (interval_seconds BETWEEN 1 AND 60),
    poll_count INTEGER NOT NULL DEFAULT 0
        CHECK (poll_count >= 0),
    last_polled_at TIMESTAMPTZ,
    approved_at TIMESTAMPTZ,
    denied_at TIMESTAMPTZ,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
