ALTER TABLE dependency_graph_node
    RENAME COLUMN candidate_executors TO candidate_assignees;

ALTER TABLE dependency_graph_node
    RENAME COLUMN executor_id TO assignee_id;

ALTER TABLE dependency_graph_node
    RENAME COLUMN executor_type TO assignee_type;
