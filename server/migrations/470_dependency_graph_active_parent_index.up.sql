CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_plan_active_parent
    ON dependency_graph_plan (workspace_id, parent_issue_id)
    WHERE status = 'active';
