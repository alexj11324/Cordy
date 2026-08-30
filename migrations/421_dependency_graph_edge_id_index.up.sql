CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_edge_id
    ON dependency_graph_edge (id);
