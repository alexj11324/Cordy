CREATE UNIQUE INDEX CONCURRENTLY uq_linear_issue_link_local
    ON linear_issue_link (workspace_id, patchbay_issue_id)
    WHERE sync_status <> 'deleted';
