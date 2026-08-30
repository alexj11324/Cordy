CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_plan_id
    ON dependency_graph_plan (id);
