CREATE INDEX CONCURRENTLY work_product_relation_issue_idx
    ON work_product_relation (workspace_id, issue_id, attached_at DESC)
    WHERE detached_at IS NULL AND issue_id IS NOT NULL;
