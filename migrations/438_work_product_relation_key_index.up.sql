CREATE UNIQUE INDEX CONCURRENTLY work_product_relation_active_key_uidx
    ON work_product_relation (work_product_id, relation_key)
    WHERE detached_at IS NULL;
