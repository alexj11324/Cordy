-- Durable opaque receive cursors for the Weixin/iLink long-poll adapter.
-- Relationships are enforced by application-layer cleanup; no FK/cascade.
CREATE TABLE channel_receive_state (
    installation_id UUID NOT NULL,
    channel_type TEXT NOT NULL,
    cursor TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
