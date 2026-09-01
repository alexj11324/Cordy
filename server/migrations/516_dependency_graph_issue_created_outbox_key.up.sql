CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_dependency_graph_issue_created_outbox_node
    ON dependency_graph_issue_created_outbox (plan_id, node_id);
