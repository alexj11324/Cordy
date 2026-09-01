CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_conflict_list
    ON linear_sync_conflict (workspace_id, status, updated_at, id);
