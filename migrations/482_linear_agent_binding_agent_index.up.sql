CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_agent_binding_agent
    ON linear_agent_binding (workspace_id, agent_id);
