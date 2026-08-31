//! Workspace-wide defaults for dependency admission and review handoff.

use patchbay_db::models::{Agent, WorkspaceIssueCategoryPolicy};
use patchbay_db::queries::{agent, workspace_issue_category_policy as policy_q};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

pub const EXECUTION_CATEGORY: &str = "in_progress";
pub const REVIEW_CATEGORY: &str = "in_review";

pub async fn list(
    pool: &PgPool,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<WorkspaceIssueCategoryPolicy>> {
    policy_q::list_workspace_issue_category_policies(pool, workspace_id).await
}

pub async fn get(
    pool: &PgPool,
    workspace_id: Uuid,
    category: &str,
) -> anyhow::Result<Option<WorkspaceIssueCategoryPolicy>> {
    policy_q::get_workspace_issue_category_policy(pool, workspace_id, category).await
}

/// Resolve a configured default only when the referenced agent still belongs
/// to this workspace and is active. A stale policy is intentionally treated as
/// no default so admission remains fail-closed instead of dispatching work to
/// an unrelated or archived agent.
pub async fn default_agent(
    pool: &PgPool,
    workspace_id: Uuid,
    category: &str,
) -> anyhow::Result<Option<Agent>> {
    let Some(policy) = get(pool, workspace_id, category).await? else {
        return Ok(None);
    };
    let agent_id = if category == REVIEW_CATEGORY {
        policy.default_reviewer_agent_id
    } else {
        policy.default_execution_agent_id
    };
    let Some(agent_id) = agent_id else {
        return Ok(None);
    };
    Ok(agent::get_agent_in_workspace(pool, agent_id, workspace_id)
        .await?
        .filter(|agent| agent.archived_at.is_none()))
}

/// Validate and persist one category policy inside the caller's transaction.
/// The referenced agents must be active, belong to the workspace, and have a
/// runtime; review and execution defaults may not point at the same agent.
pub async fn upsert(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    category: &str,
    execution_agent_id: Option<Uuid>,
    reviewer_agent_id: Option<Uuid>,
) -> anyhow::Result<WorkspaceIssueCategoryPolicy> {
    anyhow::ensure!(
        matches!(category, EXECUTION_CATEGORY | REVIEW_CATEGORY),
        "unsupported issue category policy"
    );
    if let (Some(execution), Some(reviewer)) = (execution_agent_id, reviewer_agent_id) {
        anyhow::ensure!(
            execution != reviewer,
            "execution and review agents must differ"
        );
    }
    for agent_id in [execution_agent_id, reviewer_agent_id]
        .into_iter()
        .flatten()
    {
        let found = agent::get_agent_in_workspace(&mut *executor, agent_id, workspace_id).await?;
        anyhow::ensure!(
            found.is_some_and(|agent| agent.archived_at.is_none() && agent.runtime_id.is_some()),
            "configured policy agent is unavailable"
        );
    }
    Ok(policy_q::upsert_workspace_issue_category_policy(
        &mut *executor,
        workspace_id,
        category,
        execution_agent_id,
        reviewer_agent_id,
    )
    .await?)
}
