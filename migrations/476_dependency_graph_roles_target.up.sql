-- Persist every role and the optional concrete execution target proposed by
-- Patrick.  These fields are planner metadata: the Issue row remains the
-- dispatch source of truth, while runtime/model are copied to the task target
-- snapshot when a node is admitted.
ALTER TABLE dependency_graph_node
    ADD COLUMN owner_type TEXT,
    ADD COLUMN owner_id UUID,
    ADD COLUMN reviewer_type TEXT,
    ADD COLUMN reviewer_id UUID,
    ADD COLUMN runtime_id UUID,
    ADD COLUMN model_id TEXT;

ALTER TABLE dependency_graph_node
    ADD CONSTRAINT dependency_graph_node_owner_pair_check CHECK (
        (owner_type IS NULL AND owner_id IS NULL)
        OR (owner_type = 'member' AND owner_id IS NOT NULL)
    ),
    ADD CONSTRAINT dependency_graph_node_reviewer_pair_check CHECK (
        (reviewer_type IS NULL AND reviewer_id IS NULL)
        OR (reviewer_type IN ('member', 'agent', 'team') AND reviewer_id IS NOT NULL)
    ),
    ADD CONSTRAINT dependency_graph_node_target_pair_check CHECK (
        (runtime_id IS NULL AND model_id IS NULL)
        OR (runtime_id IS NOT NULL AND model_id IS NOT NULL AND length(trim(model_id)) > 0)
    );
