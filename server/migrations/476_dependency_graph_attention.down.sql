ALTER TABLE dependency_graph_plan
    DROP COLUMN IF EXISTS attention_reason,
    DROP COLUMN IF EXISTS attention_required;
