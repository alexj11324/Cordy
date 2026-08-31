-- Explicit human-owner mapping for the Linear publish boundary. Provider
-- user IDs are opaque strings and are never inferred from Cordy UUIDs.
CREATE TABLE linear_member_binding (
    id UUID NOT NULL PRIMARY KEY,
    workspace_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    patchbay_user_id UUID NOT NULL,
    linear_user_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
