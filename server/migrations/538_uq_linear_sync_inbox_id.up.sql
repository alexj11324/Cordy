CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_sync_inbox_id
    ON linear_sync_inbox (id);
