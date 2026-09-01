//! Workspace-wide defaults for issue category admission and review handoff.

use crate::models::WorkspaceIssueCategoryPolicy;
use sqlx::Executor;
use uuid::Uuid;

const COLUMNS: &str = "workspace_id, category, default_execution_agent_id, default_reviewer_agent_id, created_at, updated_at";

pub async fn get_workspace_issue_category_policy<'e, E>(
    executor: E,
    workspace_id: Uuid,
    category: &str,
) -> anyhow::Result<Option<WorkspaceIssueCategoryPolicy>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, WorkspaceIssueCategoryPolicy>(&format!(
        "SELECT {COLUMNS} FROM workspace_issue_category_policy WHERE workspace_id = $1 AND category = $2"
    ))
    .bind(workspace_id)
    .bind(category)
    .fetch_optional(executor)
    .await?)
}

pub async fn list_workspace_issue_category_policies<'e, E>(
    executor: E,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<WorkspaceIssueCategoryPolicy>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, WorkspaceIssueCategoryPolicy>(&format!(
        "SELECT {COLUMNS} FROM workspace_issue_category_policy WHERE workspace_id = $1 ORDER BY category"
    ))
    .bind(workspace_id)
    .fetch_all(executor)
    .await?)
}

pub async fn upsert_workspace_issue_category_policy<'e, E>(
    executor: E,
    workspace_id: Uuid,
    category: &str,
    default_execution_agent_id: Option<Uuid>,
    default_reviewer_agent_id: Option<Uuid>,
) -> anyhow::Result<WorkspaceIssueCategoryPolicy>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, WorkspaceIssueCategoryPolicy>(&format!(
        "INSERT INTO workspace_issue_category_policy (workspace_id, category, default_execution_agent_id, default_reviewer_agent_id) VALUES ($1, $2, $3, $4) ON CONFLICT (workspace_id, category) DO UPDATE SET default_execution_agent_id = EXCLUDED.default_execution_agent_id, default_reviewer_agent_id = EXCLUDED.default_reviewer_agent_id, updated_at = now() RETURNING {COLUMNS}"
    ))
    .bind(workspace_id)
    .bind(category)
    .bind(default_execution_agent_id)
    .bind(default_reviewer_agent_id)
    .fetch_one(executor)
    .await?)
}

pub async fn delete_workspace_issue_category_policies<'e, E>(
    executor: E,
    workspace_id: Uuid,
) -> anyhow::Result<u64>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(
        sqlx::query("DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(executor)
            .await?
            .rows_affected(),
    )
}
