CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_connection_workspace
    ON linear_connection (workspace_id);
