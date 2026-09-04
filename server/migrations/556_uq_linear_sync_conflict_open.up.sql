CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_sync_conflict_open
    ON linear_sync_conflict (link_id, field) WHERE status = 'open';
