CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_agent_coordination_assignment_event_role
    ON agent_coordination_assignment (event_id, role);
