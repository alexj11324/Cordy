CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_project_binding_remote
    ON linear_project_binding (workspace_id, linear_project_id);
