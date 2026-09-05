CREATE TABLE linear_comment_link (
    workspace_id UUID NOT NULL,
    binding_id UUID NOT NULL,
    issue_id UUID NOT NULL,
    comment_id UUID NOT NULL,
    linear_comment_id TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('linear', 'patchbay')),
    remote_updated_at TIMESTAMPTZ,
    deleted BOOLEAN NOT NULL DEFAULT false
);
