CREATE INDEX CONCURRENTLY work_product_relation_task_idx
    ON work_product_relation (workspace_id, task_id, attached_at DESC)
    WHERE detached_at IS NULL AND task_id IS NOT NULL;
