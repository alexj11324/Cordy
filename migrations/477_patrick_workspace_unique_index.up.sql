-- Patrick is the one workspace orchestrator. The older generic system-key
-- index permits different runtime/owner combinations, so this narrower
-- invariant is required after the breaking rename.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_agent_workspace_patrick
    ON agent (workspace_id)
    WHERE system_key = 'patrick';
