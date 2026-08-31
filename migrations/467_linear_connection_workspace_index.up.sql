CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_connection_workspace
    ON linear_connection (workspace_id);
