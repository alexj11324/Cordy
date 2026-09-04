CREATE TABLE work_product (
    id                    UUID NOT NULL DEFAULT gen_random_uuid(),
    workspace_id          UUID NOT NULL,
    kind                  TEXT NOT NULL CHECK (kind IN (
        'pull_request', 'branch', 'commit', 'preview', 'artifact', 'document'
    )),
    provider              TEXT NOT NULL CHECK (char_length(trim(provider)) > 0),
    external_identity     TEXT NOT NULL CHECK (
        char_length(trim(external_identity)) > 0
        AND char_length(external_identity) <= 2048
    ),
    external_url          TEXT,
    provider_record_type  TEXT,
    provider_record_id    UUID,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
