CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_project_binding_remote
    ON linear_project_binding (connection_id, linear_project_id) WHERE status <> 'tombstone';
