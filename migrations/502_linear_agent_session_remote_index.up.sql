CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_agent_session_remote
    ON linear_agent_session (connection_id, linear_session_id);
