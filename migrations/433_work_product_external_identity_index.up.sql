CREATE UNIQUE INDEX CONCURRENTLY work_product_external_identity_uidx
    ON work_product (workspace_id, provider, external_identity);
