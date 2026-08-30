CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_node_id
    ON dependency_graph_node (id);
