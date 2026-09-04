CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_project_binding_status
    ON linear_project_binding (workspace_id, status, updated_at);
