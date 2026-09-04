CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_agent_coordination_outbox_pending
    ON agent_coordination_outbox (status, available_at, lease_expires_at);
