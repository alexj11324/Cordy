ALTER TABLE work_product_relation
    DROP CONSTRAINT IF EXISTS work_product_relation_source_check,
    DROP CONSTRAINT IF EXISTS work_product_relation_actor_type_check,
    DROP CONSTRAINT IF EXISTS work_product_relation_actor_check,
    ALTER COLUMN attached_by_id SET NOT NULL,
    ADD CONSTRAINT work_product_relation_relation_source_check CHECK (relation_source IN (
        'manual_explicit', 'task_explicit', 'execution_branch_discovery'
    )),
    ADD CONSTRAINT work_product_relation_attached_by_type_check CHECK (
        attached_by_type IN ('user', 'agent')
    ),
    ADD CONSTRAINT work_product_relation_check1 CHECK (
        (relation_source = 'manual_explicit' AND attached_by_type = 'user')
        OR
        (relation_source IN ('task_explicit', 'execution_branch_discovery')
            AND attached_by_type = 'agent'
            AND task_id IS NOT NULL)
    );
