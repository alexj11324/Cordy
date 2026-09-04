CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_dependency_graph_edge_plan_to
    ON dependency_graph_edge (plan_id, to_issue_id);
