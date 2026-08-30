CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_agent_coordination_outbox_event_key
    ON agent_coordination_outbox (event_key);
