CREATE UNIQUE INDEX CONCURRENTLY agent_task_execution_provenance_task_uidx
    ON agent_task_execution_provenance (task_id);
