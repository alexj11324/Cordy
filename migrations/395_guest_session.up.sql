-- Durable server-backed guest sessions. Token material is stored only as a
-- SHA-256 hash. Relationships are application-owned per repository policy.
CREATE TABLE guest_session (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    token_hash TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'claimed', 'revoked')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_at TIMESTAMPTZ,
    claimed_by UUID
);
