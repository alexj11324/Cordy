-- Short-lived, one-time handoff material. No raw token is persisted.
CREATE TABLE guest_session_transfer (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    guest_session_id UUID NOT NULL,
    guest_user_id UUID NOT NULL,
    token_hash TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    claimed_user_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
