CREATE TABLE desktop_auth_handoff (
    state TEXT PRIMARY KEY,
    code_challenge TEXT NOT NULL,
    callback_protocol TEXT NOT NULL,
    user_id UUID,
    code_hash TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT desktop_auth_handoff_state_length CHECK (length(state) BETWEEN 32 AND 128),
    CONSTRAINT desktop_auth_handoff_challenge_length CHECK (length(code_challenge) BETWEEN 32 AND 128),
    CONSTRAINT desktop_auth_handoff_protocol_check CHECK (callback_protocol = 'patchbay'),
    CONSTRAINT desktop_auth_handoff_code_hash_length CHECK (code_hash IS NULL OR length(code_hash) = 64),
    CONSTRAINT desktop_auth_handoff_completion_check CHECK (
        (user_id IS NULL AND code_hash IS NULL AND completed_at IS NULL)
        OR (user_id IS NOT NULL AND code_hash IS NOT NULL AND completed_at IS NOT NULL)
    )
);
