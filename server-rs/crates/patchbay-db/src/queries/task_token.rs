//! Typed SQL queries for task_token records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_task_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
    task_id: Uuid,
    agent_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    expires_at: Option<DateTime<Utc>>,
    scope: &serde_json::Value,
    parent_task_id: Option<Uuid>,
    claim_dispatched_at: Option<DateTime<Utc>>,
    delegation_fence: i64,
    on_behalf_of_user_id: Option<Uuid>,
    device_id: Option<Uuid>,
    id: Uuid,
) -> anyhow::Result<Option<TaskToken>> {
    let row = sqlx::query(
        r#"WITH claim AS (
    SELECT id, delegated_from_task_id, dispatched_at
    FROM agent_task_queue
    WHERE id = $2
      AND status IN ('dispatched', 'running', 'waiting_local_directory', 'deferred')
      AND dispatched_at IS NOT DISTINCT FROM $9::timestamptz
    FOR SHARE
), parent AS (
    SELECT token.id, token.scope, token.delegation_depth, token.delegation_fence,
           token.workspace_id, token.on_behalf_of_user_id, token.device_id
    FROM task_token token
    JOIN agent_task_queue task ON task.id = token.task_id
    WHERE token.task_id = $8
      AND token.revoked_at IS NULL
      AND token.expires_at > now()
      AND token.claim_dispatched_at = task.dispatched_at
      AND task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
    ORDER BY token.created_at DESC, token.id DESC
    LIMIT 1
    FOR SHARE OF token
), lease AS (
    SELECT
        CASE WHEN $8::uuid IS NULL THEN $7::jsonb ELSE COALESCE((
            SELECT jsonb_agg(requested.capability)
            FROM jsonb_array_elements($7::jsonb) requested(capability)
            WHERE EXISTS (
                SELECT 1
                FROM parent, jsonb_array_elements(parent.scope) bound(capability)
                WHERE bound.capability->>'action' = requested.capability->>'action'
                  AND bound.capability->>'resource_type' = requested.capability->>'resource_type'
                  AND (
                      bound.capability->>'resource_id' = '*'
                      OR bound.capability->>'resource_id' = requested.capability->>'resource_id'
                  )
            )
        ), '[]'::jsonb) END AS effective_scope,
        parent.id AS parent_id,
        parent.delegation_fence AS parent_fence,
        COALESCE(parent.delegation_depth + 1, 0) AS depth
    FROM claim
    LEFT JOIN parent ON TRUE
    WHERE $8::uuid IS NULL
       OR (claim.delegated_from_task_id = $8
           AND parent.id IS NOT NULL
           AND parent.workspace_id = $4
           AND parent.on_behalf_of_user_id IS NOT DISTINCT FROM $11
           AND parent.device_id IS NOT DISTINCT FROM $12)
), inserted AS (
    INSERT INTO task_token (
        token_hash, task_id, agent_id, workspace_id, user_id, expires_at, id,
        scope, parent_token_id, parent_fence, delegation_depth,
        delegation_fence, claim_dispatched_at, on_behalf_of_user_id, device_id
    )
    SELECT $1, $2, $3, $4, $5, $6, COALESCE($13::uuid, gen_random_uuid()),
           lease.effective_scope, lease.parent_id, lease.parent_fence, lease.depth,
           $10, $9, $11, $12
    FROM lease
    WHERE lease.depth <= 8
    ON CONFLICT (task_id, claim_dispatched_at)
        WHERE claim_dispatched_at IS NOT NULL
        DO NOTHING
    RETURNING id, token_hash, task_id, agent_id, workspace_id, user_id,
              expires_at, created_at, scope, parent_token_id, parent_fence,
              delegation_depth, delegation_fence, claim_dispatched_at,
              on_behalf_of_user_id, device_id, revoked_at, revoked_reason
)
SELECT * FROM inserted"#
    )
        .bind(token_hash)
        .bind(task_id)
        .bind(agent_id)
        .bind(workspace_id)
        .bind(user_id)
        .bind(expires_at)
        .bind(scope)
        .bind(parent_task_id)
        .bind(claim_dispatched_at)
        .bind(delegation_fence)
        .bind(on_behalf_of_user_id)
        .bind(device_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(TaskToken {
        id: row.try_get(0)?,
        token_hash: row.try_get(1)?,
        task_id: row.try_get(2)?,
        agent_id: row.try_get(3)?,
        workspace_id: row.try_get(4)?,
        user_id: row.try_get(5)?,
        expires_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
        scope: row.try_get(8)?,
        parent_token_id: row.try_get(9)?,
        parent_fence: row.try_get(10)?,
        delegation_depth: row.try_get(11)?,
        delegation_fence: row.try_get(12)?,
        claim_dispatched_at: row.try_get(13)?,
        on_behalf_of_user_id: row.try_get(14)?,
        device_id: row.try_get(15)?,
        revoked_at: row.try_get(16)?,
        revoked_reason: row.try_get(17)?,
    }))
}

pub async fn delete_expired_task_tokens(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM task_token WHERE expires_at <= now()"#)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn revoke_task_tokens_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    reason: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE task_token
SET revoked_at = COALESCE(revoked_at, now()),
    revoked_reason = COALESCE(revoked_reason, $2)
WHERE task_id = $1 AND revoked_at IS NULL"#,
    )
        .bind(task_id)
        .bind(reason)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_task_token_by_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<TaskToken>> {
    let row = sqlx::query(
        r#"WITH RECURSIVE lease_chain AS (
    SELECT token.id, token.task_id, token.agent_id, token.workspace_id,
           token.scope, token.parent_token_id, token.parent_fence,
           token.delegation_depth, token.delegation_fence,
           token.claim_dispatched_at, token.on_behalf_of_user_id,
           token.device_id, token.revoked_at, token.revoked_reason,
           token.expires_at, token.created_at, token.token_hash, token.user_id,
           task.status AS task_status, task.dispatched_at AS current_dispatched_at,
           ARRAY[token.id] AS path
    FROM task_token token
    JOIN agent_task_queue task ON task.id = token.task_id
    WHERE token.token_hash = $1
  UNION ALL
    SELECT parent.id, parent.task_id, parent.agent_id, parent.workspace_id,
           parent.scope, parent.parent_token_id, parent.parent_fence,
           parent.delegation_depth, parent.delegation_fence,
           parent.claim_dispatched_at, parent.on_behalf_of_user_id,
           parent.device_id, parent.revoked_at, parent.revoked_reason,
           parent.expires_at, parent.created_at, parent.token_hash, parent.user_id,
           task.status, task.dispatched_at, child.path || parent.id
    FROM task_token parent
    JOIN lease_chain child ON child.parent_token_id = parent.id
    JOIN agent_task_queue task ON task.id = parent.task_id
    WHERE NOT parent.id = ANY(child.path)
      AND cardinality(child.path) <= 9
), leaf AS (
    SELECT * FROM lease_chain WHERE token_hash = $1
), invalid AS (
    SELECT 1
    FROM lease_chain lease
    LEFT JOIN lease_chain parent ON parent.id = lease.parent_token_id
    WHERE lease.revoked_at IS NOT NULL
       OR lease.expires_at <= now()
       OR lease.task_status NOT IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
       OR lease.claim_dispatched_at IS DISTINCT FROM lease.current_dispatched_at
       OR lease.delegation_depth > 8
       OR (lease.parent_token_id IS NULL AND lease.delegation_depth <> 0)
       OR (lease.parent_token_id IS NOT NULL AND (
              parent.id IS NULL
              OR lease.delegation_depth <> parent.delegation_depth + 1
              OR lease.parent_fence IS DISTINCT FROM parent.delegation_fence
              OR lease.workspace_id <> parent.workspace_id
              OR lease.on_behalf_of_user_id IS DISTINCT FROM parent.on_behalf_of_user_id
              OR lease.device_id IS DISTINCT FROM parent.device_id
              OR EXISTS (
                  SELECT 1
                  FROM jsonb_array_elements(lease.scope) child_cap(capability)
                  WHERE NOT EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements(parent.scope) parent_cap(capability)
                      WHERE parent_cap.capability->>'action' = child_cap.capability->>'action'
                        AND parent_cap.capability->>'resource_type' = child_cap.capability->>'resource_type'
                        AND (
                            parent_cap.capability->>'resource_id' = '*'
                            OR parent_cap.capability->>'resource_id' = child_cap.capability->>'resource_id'
                        )
                  )
              )
          ))
    LIMIT 1
)
SELECT id, token_hash, task_id, agent_id, workspace_id, user_id,
       expires_at, created_at, scope, parent_token_id, parent_fence,
       delegation_depth, delegation_fence, claim_dispatched_at,
       on_behalf_of_user_id, device_id, revoked_at, revoked_reason
FROM leaf
WHERE NOT EXISTS (SELECT 1 FROM invalid)
  AND (SELECT count(*) FROM lease_chain) = delegation_depth + 1
  AND EXISTS (
      SELECT 1 FROM lease_chain root
      WHERE root.parent_token_id IS NULL AND root.delegation_depth = 0
  )"#
    )
        .bind(token_hash)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(TaskToken {
        id: row.try_get(0)?,
        token_hash: row.try_get(1)?,
        task_id: row.try_get(2)?,
        agent_id: row.try_get(3)?,
        workspace_id: row.try_get(4)?,
        user_id: row.try_get(5)?,
        expires_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
        scope: row.try_get(8)?,
        parent_token_id: row.try_get(9)?,
        parent_fence: row.try_get(10)?,
        delegation_depth: row.try_get(11)?,
        delegation_fence: row.try_get(12)?,
        claim_dispatched_at: row.try_get(13)?,
        on_behalf_of_user_id: row.try_get(14)?,
        device_id: row.try_get(15)?,
        revoked_at: row.try_get(16)?,
        revoked_reason: row.try_get(17)?,
    }))
}

pub async fn revoke_task_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_id: Uuid,
    reason: &str,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE task_token
SET revoked_at = now(), revoked_reason = $2
WHERE id = $1 AND revoked_at IS NULL"#,
    )
    .bind(token_id)
    .bind(reason)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn task_token_exists_for_claim(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    claim_dispatched_at: Option<DateTime<Utc>>,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS (
    SELECT 1
    FROM task_token
    WHERE task_id = $1
      AND claim_dispatched_at IS NOT DISTINCT FROM $2::timestamptz
)"#,
    )
    .bind(task_id)
    .bind(claim_dispatched_at)
    .fetch_one(executor)
    .await?)
}
