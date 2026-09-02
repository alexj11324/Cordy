ALTER TABLE work_product_relation
    DROP CONSTRAINT IF EXISTS work_product_relation_relation_source_check,
    DROP CONSTRAINT IF EXISTS work_product_relation_attached_by_type_check,
    DROP CONSTRAINT IF EXISTS work_product_relation_check1,
    ALTER COLUMN attached_by_id DROP NOT NULL,
    ADD CONSTRAINT work_product_relation_source_check CHECK (relation_source IN (
        'manual_explicit', 'task_explicit', 'execution_branch_discovery',
        'provider_discovery', 'provider_reference'
    )),
    ADD CONSTRAINT work_product_relation_actor_type_check CHECK (
        attached_by_type IN ('user', 'agent', 'system')
    ),
    ADD CONSTRAINT work_product_relation_actor_check CHECK (
        (relation_source = 'manual_explicit'
            AND attached_by_type = 'user'
            AND attached_by_id IS NOT NULL)
        OR
        (relation_source IN ('task_explicit', 'execution_branch_discovery')
            AND attached_by_type = 'agent'
            AND attached_by_id IS NOT NULL
            AND task_id IS NOT NULL)
        OR
        (relation_source IN ('provider_discovery', 'provider_reference')
            AND attached_by_type = 'system'
            AND task_id IS NULL
            AND run_id IS NULL)
    );
