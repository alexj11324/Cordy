CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_sync_outbox_id
    ON linear_sync_outbox (id);
