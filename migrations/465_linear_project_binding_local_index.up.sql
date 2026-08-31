CREATE UNIQUE INDEX CONCURRENTLY uq_linear_project_binding_local
    ON linear_project_binding (workspace_id, patchbay_project_id)
    WHERE status <> 'tombstone';
