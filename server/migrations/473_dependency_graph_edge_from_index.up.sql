CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_dependency_graph_edge_plan_from
    ON dependency_graph_edge (plan_id, from_issue_id);
