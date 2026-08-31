ALTER TABLE dependency_graph_node
    RENAME COLUMN assignee_type TO executor_type;

ALTER TABLE dependency_graph_node
    RENAME COLUMN assignee_id TO executor_id;

ALTER TABLE dependency_graph_node
    RENAME COLUMN candidate_assignees TO candidate_executors;
