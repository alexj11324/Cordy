CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_inbox_pending
    ON linear_sync_inbox (connection_id, received_at, id)
    WHERE processed_at IS NULL;
