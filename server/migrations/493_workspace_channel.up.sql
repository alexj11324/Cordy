-- In-app workspace channels. Relations to members, agents, and messages are
-- validated in the application layer; migrations in this repository do not
-- add foreign keys or cascading actions.
CREATE TABLE workspace_channel (
    id           UUID        NOT NULL DEFAULT gen_random_uuid(),
    workspace_id UUID        NOT NULL,
    name         TEXT        NOT NULL,
    slug         TEXT        NOT NULL,
    description  TEXT        NOT NULL DEFAULT '',
    created_by   UUID        NOT NULL,
    archived_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT workspace_channel_name_not_blank CHECK (length(btrim(name)) > 0),
    CONSTRAINT workspace_channel_slug_not_blank CHECK (length(btrim(slug)) > 0)
);
