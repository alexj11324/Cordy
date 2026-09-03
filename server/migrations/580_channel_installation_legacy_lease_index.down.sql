CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_channel_installation_lease
    ON channel_installation(ws_lease_expires_at) WHERE status = 'active';
