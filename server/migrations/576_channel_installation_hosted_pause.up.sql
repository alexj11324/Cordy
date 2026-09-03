-- Hosted IM installation capacity (Go port of Rust 481). A non-NULL value
-- marks an installation the host paused for capacity: it stays 'active' —
-- desired state, credentials, and bindings are preserved — but every claim,
-- lease, and supervisor list filters it out, so it receives no work and holds
-- no WebSocket. Reversible: reconcile clears it when capacity returns.
ALTER TABLE channel_installation
    ADD COLUMN IF NOT EXISTS hosted_paused_at TIMESTAMPTZ;
