CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_workspace_issue_category_policy_workspace_category
    ON workspace_issue_category_policy (workspace_id, category);
