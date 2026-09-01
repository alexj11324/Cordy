-- Attach the CONCURRENTLY-built unique indexes as the table primary keys.
ALTER TABLE agent_coordination_outbox
    ADD CONSTRAINT agent_coordination_outbox_pkey PRIMARY KEY USING INDEX agent_coordination_outbox_pkey_uidx;
ALTER TABLE agent_coordination_assignment
    ADD CONSTRAINT agent_coordination_assignment_pkey PRIMARY KEY USING INDEX agent_coordination_assignment_pkey_uidx;
