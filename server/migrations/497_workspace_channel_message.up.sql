CREATE TABLE workspace_channel_message (
    id                 UUID        NOT NULL DEFAULT gen_random_uuid(),
    workspace_id       UUID        NOT NULL,
    channel_id         UUID        NOT NULL,
    author_type        TEXT        NOT NULL,
    author_id          UUID        NOT NULL,
    content            TEXT        NOT NULL,
    parent_id          UUID,
    quoted_message_id  UUID,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT workspace_channel_message_author_type CHECK (author_type IN ('member', 'agent')),
    CONSTRAINT workspace_channel_message_content_not_blank CHECK (length(btrim(content)) > 0)
);
