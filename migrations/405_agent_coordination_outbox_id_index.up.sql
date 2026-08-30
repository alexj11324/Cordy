-- Backing index for agent_coordination_outbox's primary key, attached in 407.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS agent_coordination_outbox_pkey_uidx
    ON agent_coordination_outbox (id);
