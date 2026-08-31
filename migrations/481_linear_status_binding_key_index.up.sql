CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_status_binding_key
    ON linear_status_binding (project_binding_id, patchbay_status);
