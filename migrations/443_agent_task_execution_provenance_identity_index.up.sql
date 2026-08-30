CREATE UNIQUE INDEX CONCURRENTLY agent_task_execution_provenance_identity_uidx
    ON agent_task_execution_provenance (
        workspace_id,
        task_id,
        repo_identity,
        execution_workspace
    );
