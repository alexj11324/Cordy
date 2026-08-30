CREATE TABLE agent_task_execution_provenance (
    task_id                    UUID NOT NULL,
    workspace_id               UUID NOT NULL,
    run_id                     UUID,
    repo_identity              TEXT,
    execution_workspace        TEXT,
    head_branch                TEXT,
    head_sha                   TEXT,
    head_state                 TEXT NOT NULL CHECK (head_state IN (
        'attached', 'detached', 'default', 'unknown'
    )),
    started_at                 TIMESTAMPTZ,
    finished_at                TIMESTAMPTZ,
    discovery_status           TEXT NOT NULL DEFAULT 'not_attempted' CHECK (
        discovery_status IN (
            'not_attempted', 'unassociated', 'ambiguous', 'associated', 'ineligible'
        )
    ),
    discovery_match_count      INTEGER NOT NULL DEFAULT 0 CHECK (discovery_match_count >= 0),
    discovery_reason           TEXT,
    discovery_work_product_id  UUID,
    discovery_at               TIMESTAMPTZ,
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (repo_identity IS NOT NULL OR head_state IN ('detached', 'default', 'unknown')),
    CHECK (head_branch IS NOT NULL OR head_state IN ('detached', 'default', 'unknown'))
);
