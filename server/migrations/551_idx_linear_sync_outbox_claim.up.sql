CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_outbox_claim
    ON linear_sync_outbox (available_at, created_at, id) WHERE processed_at IS NULL AND dead_lettered_at IS NULL;
