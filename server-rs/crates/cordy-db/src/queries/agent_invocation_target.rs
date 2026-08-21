//! Port of server/pkg/db/queries/agent_invocation_target.sql (generated agent_invocation_target.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_agent_invocation_target(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    target_type: &str,
    target_id: Uuid,
    created_by: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO agent_invocation_target (agent_id, target_type, target_id, created_by)
VALUES ($1, $2, $3, $4)
ON CONFLICT (agent_id, target_type, target_id) DO UPDATE SET
    created_by = EXCLUDED.created_by,
    created_at = now()"#,
    )
    .bind(agent_id)
    .bind(target_type)
    .bind(target_id)
    .bind(created_by)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_agent_invocation_targets(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_invocation_target
WHERE agent_id = $1"#,
    )
    .bind(agent_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_agent_invocation_targets_by_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    target_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_invocation_target ait
USING agent a
WHERE ait.agent_id = a.id
  AND a.workspace_id = $1
  AND ait.target_type = 'member'
  AND ait.target_id = $2"#,
    )
    .bind(workspace_id)
    .bind(target_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_agent_invocation_targets_by_system_runtime_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_invocation_target
WHERE agent_id IN (
    SELECT id FROM agent WHERE runtime_id = $1 AND kind = 'system'
)"#,
    )
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_agent_invocation_targets(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<AgentInvocationTarget>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, target_type, target_id, created_by, created_at FROM agent_invocation_target
WHERE agent_id = $1
ORDER BY target_type ASC, created_at ASC"#
    )
        .bind(agent_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentInvocationTarget {
            id: row.try_get(0)?,
            agent_id: row.try_get(1)?,
            target_type: row.try_get(2)?,
            target_id: row.try_get(3)?,
            created_by: row.try_get(4)?,
            created_at: row.try_get(5)?,
        });
    }
    Ok(out)
}

pub async fn list_agent_invocation_targets_by_agent_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<AgentInvocationTarget>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, target_type, target_id, created_by, created_at FROM agent_invocation_target
WHERE agent_id = ANY($1::uuid[])
ORDER BY agent_id, target_type ASC, created_at ASC"#
    )
        .bind(agent_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentInvocationTarget {
            id: row.try_get(0)?,
            agent_id: row.try_get(1)?,
            target_type: row.try_get(2)?,
            target_id: row.try_get(3)?,
            created_by: row.try_get(4)?,
            created_at: row.try_get(5)?,
        });
    }
    Ok(out)
}
