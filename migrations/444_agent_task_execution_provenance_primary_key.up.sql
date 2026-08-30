ALTER TABLE agent_task_execution_provenance
    ADD CONSTRAINT agent_task_execution_provenance_pkey
    PRIMARY KEY USING INDEX agent_task_execution_provenance_identity_uidx;
