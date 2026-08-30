CREATE INDEX CONCURRENTLY agent_task_execution_provenance_branch_idx
    ON agent_task_execution_provenance (workspace_id, repo_identity, head_branch, updated_at DESC)
    WHERE head_branch IS NOT NULL AND repo_identity IS NOT NULL;
