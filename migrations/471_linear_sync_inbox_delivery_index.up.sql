CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_inbox_delivery
    ON linear_sync_inbox (connection_id, delivery_id);
