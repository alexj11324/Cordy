ALTER TABLE dependency_graph_plan
    ADD COLUMN attention_required BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN attention_reason TEXT;
