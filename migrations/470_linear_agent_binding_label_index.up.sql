CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_agent_binding_label
    ON linear_agent_binding (workspace_id, linear_label_id);
