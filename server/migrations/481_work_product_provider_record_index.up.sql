CREATE INDEX CONCURRENTLY work_product_provider_record_idx
    ON work_product (workspace_id, provider_record_type, provider_record_id)
    WHERE provider_record_type IS NOT NULL AND provider_record_id IS NOT NULL;
