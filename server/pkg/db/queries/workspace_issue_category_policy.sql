-- Workspace-wide execution and review defaults for issue status categories.

-- name: GetWorkspaceIssueCategoryPolicy :one
SELECT workspace_id,
       category,
       default_execution_agent_id,
       default_reviewer_agent_id,
       created_at,
       updated_at
FROM workspace_issue_category_policy
WHERE workspace_id = $1
  AND category = $2;

-- name: ListWorkspaceIssueCategoryPolicies :many
SELECT workspace_id,
       category,
       default_execution_agent_id,
       default_reviewer_agent_id,
       created_at,
       updated_at
FROM workspace_issue_category_policy
WHERE workspace_id = $1
ORDER BY category;

-- name: UpsertWorkspaceIssueCategoryPolicy :one
INSERT INTO workspace_issue_category_policy (
    workspace_id,
    category,
    default_execution_agent_id,
    default_reviewer_agent_id
)
VALUES ($1, $2, $3, $4)
ON CONFLICT (workspace_id, category) DO UPDATE SET
    default_execution_agent_id = EXCLUDED.default_execution_agent_id,
    default_reviewer_agent_id = EXCLUDED.default_reviewer_agent_id,
    updated_at = now()
RETURNING workspace_id,
          category,
          default_execution_agent_id,
          default_reviewer_agent_id,
          created_at,
          updated_at;
