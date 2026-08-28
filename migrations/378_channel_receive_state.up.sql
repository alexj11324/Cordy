-- Durable opaque receive cursors for polling IM adapters. There is deliberately
-- no foreign key (repository migration policy). The unique routing index is
-- built concurrently in the next single-statement migration.
CREATE TABLE channel_receive_state (
    installation_id UUID NOT NULL,
    channel_type TEXT NOT NULL,
    cursor TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
