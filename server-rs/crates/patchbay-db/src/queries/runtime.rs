//! Typed SQL queries for runtime records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn cancel_agent_tasks_by_runtime_or_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now()
WHERE (runtime_id = ANY($1::uuid[]) OR agent_id = ANY($2::uuid[]))
  AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, automation_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(runtime_ids)
        .bind(agent_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentTaskQueue {
            id: row.try_get(0)?,
            agent_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            status: row.try_get(3)?,
            priority: row.try_get(4)?,
            dispatched_at: row.try_get(5)?,
            started_at: row.try_get(6)?,
            completed_at: row.try_get(7)?,
            result: row.try_get(8)?,
            error: row.try_get(9)?,
            created_at: row.try_get(10)?,
            context: row.try_get(11)?,
            runtime_id: row.try_get(12)?,
            session_id: row.try_get(13)?,
            work_dir: row.try_get(14)?,
            trigger_comment_id: row.try_get(15)?,
            chat_session_id: row.try_get(16)?,
            automation_run_id: row.try_get(17)?,
            attempt: row.try_get(18)?,
            max_attempts: row.try_get(19)?,
            parent_task_id: row.try_get(20)?,
            failure_reason: row.try_get(21)?,
            trigger_summary: row.try_get(22)?,
            force_fresh_session: row.try_get(23)?,
            is_leader_task: row.try_get(24)?,
            wait_reason: row.try_get(25)?,
            initiator_user_id: row.try_get(26)?,
            handoff_note: row.try_get(27)?,
            prepare_lease_expires_at: row.try_get(28)?,
            team_id: row.try_get(29)?,
            runtime_mcp_overlay: row.try_get(30)?,
            escalation_for_task_id: row.try_get(31)?,
            fire_at: row.try_get(32)?,
            originator_user_id: row.try_get(33)?,
            runtime_connected_apps: row.try_get(34)?,
            coalesced_comment_ids: row.try_get(35)?,
            delivered_comment_ids: row.try_get(36)?,
            chat_input_task_id: row.try_get(37)?,
            chat_finalize_deferred_at: row.try_get(38)?,
            originator_source: row.try_get(39)?,
            delegated_from_task_id: row.try_get(40)?,
            retry_of_task_id: row.try_get(41)?,
            rerun_of_task_id: row.try_get(42)?,
            rule_version_id: row.try_get(43)?,
            trigger_evidence_kind: row.try_get(44)?,
            trigger_evidence_ref_id: row.try_get(45)?,
            accountable_user_id: row.try_get(46)?,
            session_rollout_missing: row.try_get(47)?,
            retired_session_id: row.try_get(48)?,
            quick_actions_disabled: row.try_get(49)?,
            regenerate_quick_actions_for: row.try_get(50)?,
            branch_name: row.try_get(51)?,
            durable_work_dir: row.try_get(52)?,
            execution_lane_key: row.try_get(53)?,
        });
    }
    Ok(out)
}

pub async fn count_active_agents_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row =
        sqlx::query(r#"SELECT count(*) FROM agent WHERE runtime_id = $1 AND archived_at IS NULL"#)
            .bind(runtime_id)
            .fetch_optional(executor)
            .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn count_stale_offline_runtimes_blocked_by_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    stale_before: DateTime<Utc>,
    max_rows: i32,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM (
  SELECT 1 FROM agent_runtime
  WHERE status = 'offline'
    AND last_seen_at < $1
    AND NOT EXISTS (
      SELECT 1
      FROM agent
      WHERE agent.runtime_id = agent_runtime.id
    )
    AND EXISTS (
      SELECT 1
      FROM agent_task_queue
      WHERE agent_task_queue.runtime_id = agent_runtime.id
        AND agent_task_queue.completed_at IS NULL
    )
  LIMIT $2::int
) AS blocked_runtimes"#,
    )
    .bind(stale_before)
    .bind(max_rows)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn count_tasks_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(r#"SELECT count(*) FROM agent_task_queue WHERE runtime_id = $1"#)
        .bind(runtime_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn count_undrained_tasks_by_runtime_or_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM agent_task_queue
WHERE (runtime_id = ANY($1::uuid[]) OR agent_id = ANY($2::uuid[]))
  AND completed_at IS NULL"#,
    )
    .bind(runtime_ids)
    .bind(agent_ids)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn delete_agent_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent_runtime WHERE id = $1"#)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_system_agents_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent WHERE runtime_id = $1 AND kind = 'system'"#)
        .bind(runtime_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn fail_tasks_for_offline_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    stale_before: chrono::DateTime<chrono::Utc>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH victims AS (
  SELECT task.id
  FROM agent_task_queue task
  JOIN agent_runtime runtime ON runtime.id = task.runtime_id
  WHERE task.status IN ('dispatched', 'running', 'waiting_local_directory')
    AND runtime.status = 'offline'
    AND COALESCE(runtime.last_seen_at, runtime.updated_at) < $1
  ORDER BY COALESCE(runtime.last_seen_at, runtime.updated_at), task.created_at
  LIMIT $2::int
  FOR UPDATE OF task SKIP LOCKED
)
UPDATE agent_task_queue AS task
SET status = 'failed', completed_at = now(), error = 'runtime went offline',
    failure_reason = 'runtime_offline',
    wait_reason = NULL
FROM victims
WHERE task.id = victims.id
  AND task.status IN ('dispatched', 'running', 'waiting_local_directory')
RETURNING task.id, task.agent_id, task.issue_id, task.status, task.priority, task.dispatched_at, task.started_at, task.completed_at, task.result, task.error, task.created_at, task.context, task.runtime_id, task.session_id, task.work_dir, task.trigger_comment_id, task.chat_session_id, task.automation_run_id, task.attempt, task.max_attempts, task.parent_task_id, task.failure_reason, task.trigger_summary, task.force_fresh_session, task.is_leader_task, task.wait_reason, task.initiator_user_id, task.handoff_note, task.prepare_lease_expires_at, task.team_id, task.runtime_mcp_overlay, task.escalation_for_task_id, task.fire_at, task.originator_user_id, task.runtime_connected_apps, task.coalesced_comment_ids, task.delivered_comment_ids, task.chat_input_task_id, task.chat_finalize_deferred_at, task.originator_source, task.delegated_from_task_id, task.retry_of_task_id, task.rerun_of_task_id, task.rule_version_id, task.trigger_evidence_kind, task.trigger_evidence_ref_id, task.accountable_user_id, task.session_rollout_missing, task.retired_session_id, task.quick_actions_disabled, task.regenerate_quick_actions_for, task.branch_name, task.durable_work_dir, execution_lane_key"#
    )
        .bind(stale_before)
        .bind(max_per_tick)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentTaskQueue {
            id: row.try_get(0)?,
            agent_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            status: row.try_get(3)?,
            priority: row.try_get(4)?,
            dispatched_at: row.try_get(5)?,
            started_at: row.try_get(6)?,
            completed_at: row.try_get(7)?,
            result: row.try_get(8)?,
            error: row.try_get(9)?,
            created_at: row.try_get(10)?,
            context: row.try_get(11)?,
            runtime_id: row.try_get(12)?,
            session_id: row.try_get(13)?,
            work_dir: row.try_get(14)?,
            trigger_comment_id: row.try_get(15)?,
            chat_session_id: row.try_get(16)?,
            automation_run_id: row.try_get(17)?,
            attempt: row.try_get(18)?,
            max_attempts: row.try_get(19)?,
            parent_task_id: row.try_get(20)?,
            failure_reason: row.try_get(21)?,
            trigger_summary: row.try_get(22)?,
            force_fresh_session: row.try_get(23)?,
            is_leader_task: row.try_get(24)?,
            wait_reason: row.try_get(25)?,
            initiator_user_id: row.try_get(26)?,
            handoff_note: row.try_get(27)?,
            prepare_lease_expires_at: row.try_get(28)?,
            team_id: row.try_get(29)?,
            runtime_mcp_overlay: row.try_get(30)?,
            escalation_for_task_id: row.try_get(31)?,
            fire_at: row.try_get(32)?,
            originator_user_id: row.try_get(33)?,
            runtime_connected_apps: row.try_get(34)?,
            coalesced_comment_ids: row.try_get(35)?,
            delivered_comment_ids: row.try_get(36)?,
            chat_input_task_id: row.try_get(37)?,
            chat_finalize_deferred_at: row.try_get(38)?,
            originator_source: row.try_get(39)?,
            delegated_from_task_id: row.try_get(40)?,
            retry_of_task_id: row.try_get(41)?,
            rerun_of_task_id: row.try_get(42)?,
            rule_version_id: row.try_get(43)?,
            trigger_evidence_kind: row.try_get(44)?,
            trigger_evidence_ref_id: row.try_get(45)?,
            accountable_user_id: row.try_get(46)?,
            session_rollout_missing: row.try_get(47)?,
            retired_session_id: row.try_get(48)?,
            quick_actions_disabled: row.try_get(49)?,
            regenerate_quick_actions_for: row.try_get(50)?,
            branch_name: row.try_get(51)?,
            durable_work_dir: row.try_get(52)?,
            execution_lane_key: row.try_get(53)?,
        });
    }
    Ok(out)
}

pub async fn find_legacy_runtimes_by_daemon_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    provider: &str,
    daemon_id: &str,
) -> anyhow::Result<Vec<AgentRuntime>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE workspace_id = $1
  AND provider = $2
  AND LOWER(daemon_id) = LOWER($3)"#
    )
        .bind(workspace_id)
        .bind(provider)
        .bind(daemon_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentRuntime {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            daemon_id: row.try_get(2)?,
            name: row.try_get(3)?,
            runtime_mode: row.try_get(4)?,
            provider: row.try_get(5)?,
            status: row.try_get(6)?,
            device_info: row.try_get(7)?,
            metadata: row.try_get(8)?,
            last_seen_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            owner_id: row.try_get(12)?,
            legacy_daemon_id: row.try_get(13)?,
            visibility: row.try_get(14)?,
            profile_id: row.try_get(15)?,
            custom_name: row.try_get(16)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ForceOfflineRuntimesByIDsRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub daemon_id: Option<String>,
    pub provider: String,
}

pub async fn force_offline_runtimes_by_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<ForceOfflineRuntimesByIDsRow>> {
    let rows = sqlx::query(
        r#"UPDATE agent_runtime
SET status = 'offline', updated_at = now()
WHERE id = ANY($1::uuid[]) AND status = 'online'
RETURNING id, workspace_id, owner_id, daemon_id, provider"#,
    )
    .bind(runtime_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ForceOfflineRuntimesByIDsRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            owner_id: row.try_get(2)?,
            daemon_id: row.try_get(3)?,
            provider: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn get_agent_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentRuntime>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentRuntime {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
    }))
}

pub async fn get_agent_runtime_for_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<AgentRuntime>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentRuntime {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
    }))
}

pub async fn get_agent_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    ids: Vec<Uuid>,
) -> anyhow::Result<Vec<AgentRuntime>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE id = ANY($1::uuid[])"#
    )
        .bind(ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentRuntime {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            daemon_id: row.try_get(2)?,
            name: row.try_get(3)?,
            runtime_mode: row.try_get(4)?,
            provider: row.try_get(5)?,
            status: row.try_get(6)?,
            device_info: row.try_get(7)?,
            metadata: row.try_get(8)?,
            last_seen_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            owner_id: row.try_get(12)?,
            legacy_daemon_id: row.try_get(13)?,
            visibility: row.try_get(14)?,
            profile_id: row.try_get(15)?,
            custom_name: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn is_agent_runtime_eligible_for_gc(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    stale_before: DateTime<Utc>,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
  SELECT 1 FROM agent_runtime
  WHERE agent_runtime.id = $1
    AND status = 'offline'
    AND last_seen_at < $2
    AND NOT EXISTS (
      SELECT 1
      FROM agent
      WHERE agent.runtime_id = agent_runtime.id
    )
) AS eligible"#,
    )
    .bind(id)
    .bind(stale_before)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_agent_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<AgentRuntime>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE workspace_id = $1
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentRuntime {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            daemon_id: row.try_get(2)?,
            name: row.try_get(3)?,
            runtime_mode: row.try_get(4)?,
            provider: row.try_get(5)?,
            status: row.try_get(6)?,
            device_info: row.try_get(7)?,
            metadata: row.try_get(8)?,
            last_seen_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            owner_id: row.try_get(12)?,
            legacy_daemon_id: row.try_get(13)?,
            visibility: row.try_get(14)?,
            profile_id: row.try_get(15)?,
            custom_name: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn list_agent_runtimes_by_owner(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    owner_id: Uuid,
) -> anyhow::Result<Vec<AgentRuntime>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE workspace_id = $1 AND owner_id = $2
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .bind(owner_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentRuntime {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            daemon_id: row.try_get(2)?,
            name: row.try_get(3)?,
            runtime_mode: row.try_get(4)?,
            provider: row.try_get(5)?,
            status: row.try_get(6)?,
            device_info: row.try_get(7)?,
            metadata: row.try_get(8)?,
            last_seen_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            owner_id: row.try_get(12)?,
            legacy_daemon_id: row.try_get(13)?,
            visibility: row.try_get(14)?,
            profile_id: row.try_get(15)?,
            custom_name: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn list_daemon_custom_names(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    daemon_id: Option<&str>,
    exclude_id: Uuid,
) -> anyhow::Result<Vec<Option<String>>> {
    let rows = sqlx::query(
        r#"SELECT custom_name FROM agent_runtime
WHERE workspace_id = $1
  AND daemon_id = $2
  AND id <> $3"#,
    )
    .bind(workspace_id)
    .bind(daemon_id)
    .bind(exclude_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_stale_offline_runtime_gc_candidates(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    stale_before: DateTime<Utc>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_runtime
WHERE status = 'offline'
  AND last_seen_at < $1
  AND NOT EXISTS (
    SELECT 1
    FROM agent
    WHERE agent.runtime_id = agent_runtime.id
  )
  AND NOT EXISTS (
    SELECT 1
    FROM agent_task_queue
    WHERE agent_task_queue.runtime_id = agent_runtime.id
      AND agent_task_queue.completed_at IS NULL
  )
ORDER BY last_seen_at ASC, id ASC
LIMIT $2::int"#,
    )
    .bind(stale_before)
    .bind(max_per_tick)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn lock_agent_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentRuntime>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name FROM agent_runtime
WHERE id = $1
FOR UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentRuntime {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
    }))
}

pub async fn lock_runtimes_for_merge(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_runtime
WHERE id = ANY($1::uuid[])
ORDER BY id
FOR UPDATE"#,
    )
    .bind(runtime_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn lock_workspace_for_runtime_merge(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT 1 FROM workspace w
WHERE w.id IN (
    SELECT r.workspace_id FROM agent_runtime r WHERE r.id = ANY($1::uuid[])
)
ORDER BY w.id
FOR KEY SHARE"#,
    )
    .bind(runtime_ids)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn mark_agent_runtime_online(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentRuntime>> {
    let row = sqlx::query(
        r#"UPDATE agent_runtime
SET status = 'online', last_seen_at = now(), updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentRuntime {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MarkRuntimesOfflineByIDsRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub daemon_id: Option<String>,
    pub provider: String,
}

pub async fn mark_runtimes_offline_by_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    ids: Vec<Uuid>,
    stale_before: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<Vec<MarkRuntimesOfflineByIDsRow>> {
    let rows = sqlx::query(
        r#"UPDATE agent_runtime
SET status = 'offline', updated_at = now()
WHERE status = 'online'
  AND id = ANY($1::uuid[])
  AND last_seen_at < $2
RETURNING id, workspace_id, owner_id, daemon_id, provider"#,
    )
    .bind(ids)
    .bind(stale_before)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(MarkRuntimesOfflineByIDsRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            owner_id: row.try_get(2)?,
            daemon_id: row.try_get(3)?,
            provider: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn reassign_agents_to_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    new_runtime_id: Uuid,
    old_runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent
SET runtime_id = $1
WHERE runtime_id = $2"#,
    )
    .bind(new_runtime_id)
    .bind(old_runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReassignTasksToRuntimeRow {
    pub fence_ok: bool,
    pub reassigned_tasks: i64,
}

pub async fn reassign_tasks_to_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    new_runtime_id: Uuid,
    old_runtime_id: Uuid,
) -> anyhow::Result<Option<ReassignTasksToRuntimeRow>> {
    let row = sqlx::query(
        r#"WITH fence AS MATERIALIZED (
    -- Once per statement rather than once per row: the predicate is VOLATILE, so
    -- calling it from the WHERE clause of a bulk UPDATE would re-run it for every
    -- candidate row.
    SELECT lock_task_owner_rows(NULL, NULL, $1) AS ok
),
reassigned AS (
    UPDATE agent_task_queue
    SET runtime_id = $1
    WHERE runtime_id = $2
      AND (SELECT ok FROM fence)
    RETURNING id
)
SELECT
    (SELECT ok FROM fence) AS fence_ok,
    (SELECT count(*) FROM reassigned) AS reassigned_tasks"#,
    )
    .bind(new_runtime_id)
    .bind(old_runtime_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ReassignTasksToRuntimeRow {
        fence_ok: row.try_get(0)?,
        reassigned_tasks: row.try_get(1)?,
    }))
}

pub async fn record_runtime_legacy_daemon_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    legacy_daemon_id: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_runtime
SET legacy_daemon_id = COALESCE(legacy_daemon_id, $2)
WHERE id = $1"#,
    )
    .bind(id)
    .bind(legacy_daemon_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SelectStaleOnlineRuntimesRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub daemon_id: Option<String>,
    pub provider: String,
}

pub async fn select_stale_online_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    stale_before: chrono::DateTime<chrono::Utc>,
    max_rows: i32,
) -> anyhow::Result<Vec<SelectStaleOnlineRuntimesRow>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, owner_id, daemon_id, provider FROM agent_runtime
WHERE status = 'online'
  AND last_seen_at < $1
ORDER BY last_seen_at, id
LIMIT $2"#,
    )
    .bind(stale_before)
    .bind(max_rows)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(SelectStaleOnlineRuntimesRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            owner_id: row.try_get(2)?,
            daemon_id: row.try_get(3)?,
            provider: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn set_agent_runtime_offline(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_runtime
SET status = 'offline', updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_agent_runtime_offline_with_reason(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    offline_reason: &serde_json::Value,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_runtime
SET status = 'offline',
    metadata = metadata || jsonb_build_object('offline_reason', $2::jsonb),
    updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(offline_reason)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn touch_agent_runtime_last_seen(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_runtime
SET last_seen_at = now()
WHERE id = $1 AND status = 'online'"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn touch_agent_runtimes_last_seen_batch(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    ids: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_runtime
SET last_seen_at = now()
WHERE id = ANY($1::uuid[]) AND status = 'online'"#,
    )
    .bind(ids)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn unbind_tasks_from_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET runtime_id = NULL
WHERE runtime_id = $1 AND completed_at IS NOT NULL"#,
    )
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn unbind_user_agents_from_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"UPDATE agent
SET runtime_id = NULL, updated_at = now()
WHERE runtime_id = $1 AND kind = 'user'
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(runtime_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Agent {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            avatar_url: row.try_get(3)?,
            runtime_mode: row.try_get(4)?,
            runtime_config: row.try_get(5)?,
            visibility: row.try_get(6)?,
            status: row.try_get(7)?,
            max_concurrent_tasks: row.try_get(8)?,
            owner_id: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            description: row.try_get(12)?,
            runtime_id: row.try_get(13)?,
            instructions: row.try_get(14)?,
            archived_at: row.try_get(15)?,
            archived_by: row.try_get(16)?,
            custom_env: row.try_get(17)?,
            custom_args: row.try_get(18)?,
            mcp_config: row.try_get(19)?,
            model: row.try_get(20)?,
            thinking_level: row.try_get(21)?,
            composio_toolkit_allowlist: row.try_get(22)?,
            permission_mode: row.try_get(23)?,
            kind: row.try_get(24)?,
            system_key: row.try_get(25)?,
            disabled_runtime_skills: row.try_get(26)?,
            service_tier: row.try_get(27)?,
        });
    }
    Ok(out)
}

pub async fn update_agent_runtime_custom_name(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    custom_name: Option<&str>,
    id: Uuid,
) -> anyhow::Result<Option<AgentRuntime>> {
    let row = sqlx::query(
        r#"UPDATE agent_runtime
SET custom_name = $1, updated_at = now()
WHERE id = $2
RETURNING id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name"#
    )
        .bind(custom_name)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentRuntime {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
    }))
}

pub async fn update_agent_runtime_custom_name_by_daemon(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    custom_name: Option<&str>,
    workspace_id: Uuid,
    daemon_id: Option<&str>,
    owner_id: Option<Uuid>,
) -> anyhow::Result<Vec<AgentRuntime>> {
    let rows = sqlx::query(
        r#"UPDATE agent_runtime
SET custom_name = $1, updated_at = now()
WHERE workspace_id = $2
  AND daemon_id = $3
  AND ($4::uuid IS NULL OR owner_id = $4)
RETURNING id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name"#
    )
        .bind(custom_name)
        .bind(workspace_id)
        .bind(daemon_id)
        .bind(owner_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AgentRuntime {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            daemon_id: row.try_get(2)?,
            name: row.try_get(3)?,
            runtime_mode: row.try_get(4)?,
            provider: row.try_get(5)?,
            status: row.try_get(6)?,
            device_info: row.try_get(7)?,
            metadata: row.try_get(8)?,
            last_seen_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            owner_id: row.try_get(12)?,
            legacy_daemon_id: row.try_get(13)?,
            visibility: row.try_get(14)?,
            profile_id: row.try_get(15)?,
            custom_name: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn update_agent_runtime_visibility(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    visibility: &str,
    id: Uuid,
) -> anyhow::Result<Option<AgentRuntime>> {
    let row = sqlx::query(
        r#"UPDATE agent_runtime
SET visibility = $1, updated_at = now()
WHERE id = $2
RETURNING id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name"#
    )
        .bind(visibility)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentRuntime {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpsertAgentRuntimeRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub daemon_id: Option<String>,
    pub name: String,
    pub runtime_mode: String,
    pub provider: String,
    pub status: String,
    pub device_info: String,
    pub metadata: Option<serde_json::Value>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub owner_id: Option<Uuid>,
    pub legacy_daemon_id: Option<String>,
    pub visibility: String,
    pub profile_id: Option<Uuid>,
    pub custom_name: Option<String>,
    pub inserted: bool,
}

pub async fn upsert_agent_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    daemon_id: Option<&str>,
    name: &str,
    runtime_mode: &str,
    provider: &str,
    status: &str,
    device_info: &str,
    metadata: &serde_json::Value,
    owner_id: Option<Uuid>,
) -> anyhow::Result<Option<UpsertAgentRuntimeRow>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_runtime (
    workspace_id,
    daemon_id,
    name,
    runtime_mode,
    provider,
    status,
    device_info,
    metadata,
    owner_id,
    last_seen_at
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
ON CONFLICT (workspace_id, daemon_id, provider) WHERE profile_id IS NULL
DO UPDATE SET
    name = EXCLUDED.name,
    runtime_mode = EXCLUDED.runtime_mode,
    status = EXCLUDED.status,
    device_info = EXCLUDED.device_info,
    metadata = EXCLUDED.metadata,
    owner_id = COALESCE(EXCLUDED.owner_id, agent_runtime.owner_id),
    last_seen_at = now(),
    updated_at = now()
RETURNING id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name, (xmax = 0) AS inserted"#
    )
        .bind(workspace_id)
        .bind(daemon_id)
        .bind(name)
        .bind(runtime_mode)
        .bind(provider)
        .bind(status)
        .bind(device_info)
        .bind(metadata)
        .bind(owner_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(UpsertAgentRuntimeRow {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
        inserted: row.try_get(17)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpsertAgentRuntimeWithProfileRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub daemon_id: Option<String>,
    pub name: String,
    pub runtime_mode: String,
    pub provider: String,
    pub status: String,
    pub device_info: String,
    pub metadata: Option<serde_json::Value>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub owner_id: Option<Uuid>,
    pub legacy_daemon_id: Option<String>,
    pub visibility: String,
    pub profile_id: Option<Uuid>,
    pub custom_name: Option<String>,
    pub inserted: bool,
}

pub async fn upsert_agent_runtime_with_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    daemon_id: Option<&str>,
    name: &str,
    runtime_mode: &str,
    provider: &str,
    status: &str,
    device_info: &str,
    metadata: &serde_json::Value,
    owner_id: Option<Uuid>,
    profile_id: Uuid,
) -> anyhow::Result<Option<UpsertAgentRuntimeWithProfileRow>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_runtime (
    workspace_id,
    daemon_id,
    name,
    runtime_mode,
    provider,
    status,
    device_info,
    metadata,
    owner_id,
    profile_id,
    last_seen_at
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
ON CONFLICT (workspace_id, daemon_id, profile_id) WHERE profile_id IS NOT NULL
DO UPDATE SET
    name = EXCLUDED.name,
    runtime_mode = EXCLUDED.runtime_mode,
    provider = EXCLUDED.provider,
    status = EXCLUDED.status,
    device_info = EXCLUDED.device_info,
    metadata = EXCLUDED.metadata,
    owner_id = COALESCE(EXCLUDED.owner_id, agent_runtime.owner_id),
    last_seen_at = now(),
    updated_at = now()
RETURNING id, workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at, created_at, updated_at, owner_id, legacy_daemon_id, visibility, profile_id, custom_name, (xmax = 0) AS inserted"#
    )
        .bind(workspace_id)
        .bind(daemon_id)
        .bind(name)
        .bind(runtime_mode)
        .bind(provider)
        .bind(status)
        .bind(device_info)
        .bind(metadata)
        .bind(owner_id)
        .bind(profile_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(UpsertAgentRuntimeWithProfileRow {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        daemon_id: row.try_get(2)?,
        name: row.try_get(3)?,
        runtime_mode: row.try_get(4)?,
        provider: row.try_get(5)?,
        status: row.try_get(6)?,
        device_info: row.try_get(7)?,
        metadata: row.try_get(8)?,
        last_seen_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        owner_id: row.try_get(12)?,
        legacy_daemon_id: row.try_get(13)?,
        visibility: row.try_get(14)?,
        profile_id: row.try_get(15)?,
        custom_name: row.try_get(16)?,
        inserted: row.try_get(17)?,
    }))
}
