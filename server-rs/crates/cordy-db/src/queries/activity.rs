//! Port of server/pkg/db/queries/activity.sql (generated activity.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CountAssigneeChangesByActorRow {
    pub assignee_type: serde_json::Value,
    pub assignee_id: serde_json::Value,
    pub frequency: i64,
}

pub async fn count_assignee_changes_by_actor(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    actor_id: Uuid,
) -> anyhow::Result<Vec<CountAssigneeChangesByActorRow>> {
    let rows = sqlx::query(
        r#"SELECT
  details->>'to_type' as assignee_type,
  details->>'to_id' as assignee_id,
  COUNT(*)::bigint as frequency
FROM activity_log
WHERE workspace_id = $1
  AND actor_id = $2
  AND actor_type = 'member'
  AND action = 'assignee_changed'
  AND details->>'to_type' IS NOT NULL
  AND details->>'to_id' IS NOT NULL
GROUP BY details->>'to_type', details->>'to_id'"#,
    )
    .bind(workspace_id)
    .bind(actor_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(CountAssigneeChangesByActorRow {
            assignee_type: row.try_get(0)?,
            assignee_id: row.try_get(1)?,
            frequency: row.try_get(2)?,
        });
    }
    Ok(out)
}

pub async fn create_activity(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_id: Uuid,
    actor_type: Option<&str>,
    actor_id: Uuid,
    action: &str,
    details: &serde_json::Value,
    id: Uuid,
) -> anyhow::Result<Option<ActivityLog>> {
    let row = sqlx::query(
        r#"INSERT INTO activity_log (
    workspace_id, issue_id, actor_type, actor_id, action, details, id
) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7::uuid, gen_random_uuid()))
RETURNING id, workspace_id, issue_id, actor_type, actor_id, action, details, created_at"#,
    )
    .bind(workspace_id)
    .bind(issue_id)
    .bind(actor_type)
    .bind(actor_id)
    .bind(action)
    .bind(details)
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ActivityLog {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        issue_id: row.try_get(2)?,
        actor_type: row.try_get(3)?,
        actor_id: row.try_get(4)?,
        action: row.try_get(5)?,
        details: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

pub async fn get_activity(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<ActivityLog>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, actor_type, actor_id, action, details, created_at FROM activity_log
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ActivityLog {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        issue_id: row.try_get(2)?,
        actor_type: row.try_get(3)?,
        actor_id: row.try_get(4)?,
        action: row.try_get(5)?,
        details: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

pub async fn has_squad_leader_no_action_evaluation_for_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
    task_id: &str,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
  SELECT 1
  FROM activity_log
  WHERE issue_id = $1
    AND actor_type = 'agent'
    AND actor_id = $2
    AND action = 'squad_leader_evaluated'
    AND details->>'outcome' = 'no_action'
    AND details->>'task_id' = $3::text
) AS exists"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .bind(task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_activities_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<ActivityLog>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, actor_type, actor_id, action, details, created_at FROM (
    SELECT id, workspace_id, issue_id, actor_type, actor_id, action, details, created_at FROM activity_log
    WHERE issue_id = $1
    ORDER BY created_at DESC, id DESC
    LIMIT $2
) AS recent
ORDER BY created_at ASC, id ASC"#
    )
        .bind(issue_id)
        .bind(limit)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ActivityLog {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            actor_type: row.try_get(3)?,
            actor_id: row.try_get(4)?,
            action: row.try_get(5)?,
            details: row.try_get(6)?,
            created_at: row.try_get(7)?,
        });
    }
    Ok(out)
}
