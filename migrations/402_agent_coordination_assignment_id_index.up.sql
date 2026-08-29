-- Backing index for agent_coordination_assignment's primary key, attached in 403.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS agent_coordination_assignment_pkey_uidx
    ON agent_coordination_assignment (id);
