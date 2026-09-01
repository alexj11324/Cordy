-- Explicit human-owner mapping for the Linear publish boundary. Provider
-- user IDs are opaque strings and are never inferred from Patchbay UUIDs.
CREATE TABLE linear_member_binding (
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    patchbay_user_id UUID NOT NULL,
    linear_user_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
