CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_node_plan_temp
    ON dependency_graph_node (plan_id, temp_id);
