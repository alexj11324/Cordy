CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_outbox_correlation
    ON linear_sync_outbox (workspace_id, correlation_id);
