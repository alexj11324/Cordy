CREATE INDEX CONCURRENTLY work_product_relation_product_idx
    ON work_product_relation (workspace_id, work_product_id, attached_at DESC)
    WHERE detached_at IS NULL;
