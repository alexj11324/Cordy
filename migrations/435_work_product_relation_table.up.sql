CREATE TABLE work_product_relation (
    id                 UUID NOT NULL DEFAULT gen_random_uuid(),
    workspace_id       UUID NOT NULL,
    work_product_id    UUID NOT NULL,
    issue_id           UUID,
    task_id            UUID,
    run_id             UUID,
    relation_key       TEXT NOT NULL CHECK (char_length(trim(relation_key)) > 0),
    relation_source    TEXT NOT NULL CHECK (relation_source IN (
        'manual_explicit', 'task_explicit', 'execution_branch_discovery'
    )),
    attached_by_type   TEXT NOT NULL CHECK (attached_by_type IN ('user', 'agent')),
    attached_by_id     UUID NOT NULL,
    attached_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    close_intent       BOOLEAN NOT NULL DEFAULT FALSE,
    detached_at        TIMESTAMPTZ,
    detached_by_type   TEXT,
    detached_by_id     UUID,
    detached_task_id   UUID,
    detached_run_id    UUID,
    CHECK (issue_id IS NOT NULL OR task_id IS NOT NULL OR run_id IS NOT NULL),
    CHECK (
        (relation_source = 'manual_explicit'
            AND attached_by_type = 'user')
        OR
        (relation_source IN ('task_explicit', 'execution_branch_discovery')
            AND attached_by_type = 'agent'
            AND task_id IS NOT NULL)
    ),
    CHECK (run_id IS NULL OR task_id IS NOT NULL),
    CHECK (
        (detached_at IS NULL AND detached_by_type IS NULL AND detached_by_id IS NULL)
        OR (detached_at IS NOT NULL AND detached_by_type IS NOT NULL AND detached_by_id IS NOT NULL)
    )
);
