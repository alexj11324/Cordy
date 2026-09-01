ALTER TABLE dependency_graph_node
    DROP CONSTRAINT IF EXISTS dependency_graph_node_owner_pair_check,
    DROP CONSTRAINT IF EXISTS dependency_graph_node_reviewer_pair_check,
    DROP CONSTRAINT IF EXISTS dependency_graph_node_target_pair_check;

ALTER TABLE dependency_graph_node
    DROP COLUMN IF EXISTS owner_type,
    DROP COLUMN IF EXISTS owner_id,
    DROP COLUMN IF EXISTS reviewer_type,
    DROP COLUMN IF EXISTS reviewer_id,
    DROP COLUMN IF EXISTS runtime_id,
    DROP COLUMN IF EXISTS model_id;
