CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_plan_idempotency
    ON dependency_graph_plan (workspace_id, idempotency_key);
