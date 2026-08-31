-- Dependency graph nodes use the same executor vocabulary as Issue and task
-- APIs. Migration 418 is immutable history; rename its persisted columns in
-- this forward migration so fresh and upgraded databases converge on the
-- breaking contract without a dual-read compatibility layer.
ALTER TABLE dependency_graph_node
    RENAME COLUMN assignee_type TO executor_type;

ALTER TABLE dependency_graph_node
    RENAME COLUMN assignee_id TO executor_id;

ALTER TABLE dependency_graph_node
    RENAME COLUMN candidate_assignees TO candidate_executors;
