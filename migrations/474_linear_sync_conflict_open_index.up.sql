CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_conflict_open
    ON linear_sync_conflict (workspace_id, status, created_at DESC);
