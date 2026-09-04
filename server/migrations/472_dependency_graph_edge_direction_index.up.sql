CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_edge_plan_direction
    ON dependency_graph_edge (plan_id, from_issue_id, to_issue_id);
