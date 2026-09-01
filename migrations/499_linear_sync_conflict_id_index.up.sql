CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_sync_conflict_id
    ON linear_sync_conflict (id);
