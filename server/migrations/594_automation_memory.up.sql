CREATE TABLE automation_memory (
    automation_id uuid NOT NULL,
    name text NOT NULL,
    content text NOT NULL,
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    deleted boolean NOT NULL DEFAULT false,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (octet_length(content) <= 65536),
    CHECK (octet_length(name) <= 128)
);
