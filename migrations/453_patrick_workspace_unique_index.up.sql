-- There is one canonical built-in orchestrator per workspace.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_agent_workspace_patrick
    ON agent (workspace_id)
    WHERE system_key = 'patrick';
