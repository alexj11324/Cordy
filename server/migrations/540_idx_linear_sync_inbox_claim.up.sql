CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_inbox_claim
    ON linear_sync_inbox (available_at, received_at, id) WHERE processed_at IS NULL AND dead_lettered_at IS NULL;
