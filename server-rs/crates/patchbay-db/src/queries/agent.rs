//! Typed SQL queries for agent records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn acknowledge_exhausted_delegated_failure_recovery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    failed_task_id: Uuid,
    max_attempts: i32,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue AS acknowledged
SET delivered_comment_ids = (
    SELECT COALESCE(array_agg(DISTINCT receipt.id), '{}')::uuid[]
    FROM unnest(array_append(acknowledged.delivered_comment_ids, $1::uuid)) AS receipt(id)
)
WHERE acknowledged.id = (
    SELECT attempt.id
    FROM agent_task_queue attempt
    WHERE attempt.trigger_evidence_kind = 'delegated_failure'
      AND attempt.trigger_evidence_ref_id = $2
    ORDER BY attempt.created_at DESC, attempt.id DESC
    LIMIT 1
    FOR UPDATE
)
  AND (
      SELECT count(*)
      FROM agent_task_queue attempt_count
      WHERE attempt_count.trigger_evidence_kind = 'delegated_failure'
        AND attempt_count.trigger_evidence_ref_id = $2
  ) >= $3::int
RETURNING acknowledged.id, acknowledged.agent_id, acknowledged.issue_id, acknowledged.status, acknowledged.priority, acknowledged.dispatched_at, acknowledged.started_at, acknowledged.completed_at, acknowledged.result, acknowledged.error, acknowledged.created_at, acknowledged.context, acknowledged.runtime_id, acknowledged.session_id, acknowledged.work_dir, acknowledged.trigger_comment_id, acknowledged.chat_session_id, acknowledged.autopilot_run_id, acknowledged.attempt, acknowledged.max_attempts, acknowledged.parent_task_id, acknowledged.failure_reason, acknowledged.trigger_summary, acknowledged.force_fresh_session, acknowledged.is_leader_task, acknowledged.wait_reason, acknowledged.initiator_user_id, acknowledged.handoff_note, acknowledged.prepare_lease_expires_at, acknowledged.team_id, acknowledged.runtime_mcp_overlay, acknowledged.escalation_for_task_id, acknowledged.fire_at, acknowledged.originator_user_id, acknowledged.runtime_connected_apps, acknowledged.coalesced_comment_ids, acknowledged.delivered_comment_ids, acknowledged.chat_input_task_id, acknowledged.chat_finalize_deferred_at, acknowledged.originator_source, acknowledged.delegated_from_task_id, acknowledged.retry_of_task_id, acknowledged.rerun_of_task_id, acknowledged.rule_version_id, acknowledged.trigger_evidence_kind, acknowledged.trigger_evidence_ref_id, acknowledged.accountable_user_id, acknowledged.session_rollout_missing, acknowledged.retired_session_id, acknowledged.quick_actions_disabled, acknowledged.regenerate_quick_actions_for, acknowledged.branch_name, acknowledged.durable_work_dir, execution_lane_key"#
    )
        .bind(comment_id)
        .bind(failed_task_id)
        .bind(max_attempts)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn archive_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    archived_by: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET archived_at = now(), archived_by = $2, updated_at = now()
WHERE id = $1 AND archived_at IS NULL
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .bind(archived_by)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn archive_agents_by_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    archived_by: Uuid,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"UPDATE agent
SET archived_at = now(), archived_by = $1, updated_at = now()
WHERE id = ANY($2::uuid[]) AND archived_at IS NULL
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(archived_by)
        .bind(agent_ids)
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

pub async fn archive_agents_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    archived_by: Uuid,
    runtime_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"UPDATE agent
SET archived_at = now(), archived_by = $1, updated_at = now()
WHERE runtime_id = ANY($2::uuid[]) AND archived_at IS NULL
  AND (system_key IS NULL OR system_key = '')
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(archived_by)
        .bind(runtime_ids)
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

pub async fn cancel_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn cancel_agent_task_by_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue AS task
SET status = 'cancelled',
    completed_at = now(),
    prepare_lease_expires_at = NULL,
    delivered_comment_ids = CASE
      -- Chat and ordinary issue tasks almost never carry a delegated-failure
      -- recovery signal. Keep their high-frequency user-cancel path to a
      -- no-join update; only validate task lineage after the cheap comment
      -- shape probe finds a possible recovery signal.
      WHEN task.trigger_comment_id IS NULL
       AND COALESCE(cardinality(task.coalesced_comment_ids), 0) = 0
        THEN task.delivered_comment_ids
      WHEN NOT EXISTS (
        SELECT 1
        FROM comment recovery_signal
        WHERE (
            recovery_signal.id = task.trigger_comment_id
            OR recovery_signal.id = ANY(task.coalesced_comment_ids)
        )
          AND recovery_signal.author_type = 'system'
          AND recovery_signal.type = 'progress_update'
          AND recovery_signal.source_task_id IS NOT NULL
      ) THEN task.delivered_comment_ids
      ELSE (
        SELECT COALESCE(array_agg(DISTINCT receipt.id), '{}')::uuid[]
        FROM unnest(array_cat(
            task.delivered_comment_ids,
            ARRAY(
                SELECT recovery.id
                FROM comment recovery
                JOIN agent_task_queue failed ON failed.id = recovery.source_task_id
                JOIN agent_task_queue source ON source.id = failed.delegated_from_task_id
                WHERE (
                    recovery.id = task.trigger_comment_id
                    OR recovery.id = ANY(task.coalesced_comment_ids)
                )
                  AND recovery.author_type = 'system'
                  AND recovery.type = 'progress_update'
                  AND recovery.source_task_id IS NOT NULL
                  AND failed.status = 'failed'
                  AND failed.delegated_from_task_id IS NOT NULL
                  AND failed.autopilot_run_id IS NULL
                  AND failed.trigger_evidence_kind IS DISTINCT FROM 'delegated_failure'
                  AND source.autopilot_run_id IS NULL
                  AND source.issue_id = task.issue_id
                  AND source.agent_id = task.agent_id
                  AND recovery.issue_id = source.issue_id
            )
        )) AS receipt(id)
      )
    END
WHERE task.id = $1
  AND task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING task.id, task.agent_id, task.issue_id, task.status, task.priority, task.dispatched_at, task.started_at, task.completed_at, task.result, task.error, task.created_at, task.context, task.runtime_id, task.session_id, task.work_dir, task.trigger_comment_id, task.chat_session_id, task.autopilot_run_id, task.attempt, task.max_attempts, task.parent_task_id, task.failure_reason, task.trigger_summary, task.force_fresh_session, task.is_leader_task, task.wait_reason, task.initiator_user_id, task.handoff_note, task.prepare_lease_expires_at, task.team_id, task.runtime_mcp_overlay, task.escalation_for_task_id, task.fire_at, task.originator_user_id, task.runtime_connected_apps, task.coalesced_comment_ids, task.delivered_comment_ids, task.chat_input_task_id, task.chat_finalize_deferred_at, task.originator_source, task.delegated_from_task_id, task.retry_of_task_id, task.rerun_of_task_id, task.rule_version_id, task.trigger_evidence_kind, task.trigger_evidence_ref_id, task.accountable_user_id, task.session_rollout_missing, task.retired_session_id, task.quick_actions_disabled, task.regenerate_quick_actions_for, task.branch_name, task.durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn cancel_agent_task_with_reason(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    error: Option<&str>,
    failure_reason: Option<&str>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled',
    completed_at = now(),
    error = $1,
    failure_reason = $2,
    prepare_lease_expires_at = NULL
WHERE id = $3 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(error)
        .bind(failure_reason)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn cancel_agent_tasks_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE agent_id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(agent_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_agent_tasks_by_chat_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE chat_session_id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(chat_session_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_agent_tasks_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE issue_id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(issue_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_agent_tasks_by_trigger_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    trigger_comment_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE (trigger_comment_id = $1 OR $1 = ANY(coalesced_comment_ids))
  AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(trigger_comment_id)
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
            autopilot_run_id: row.try_get(17)?,
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct CancelDeferredEscalationsForIssueAgentRow {
    pub id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub status: String,
    pub priority: i32,
    pub dispatched_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub context: Option<serde_json::Value>,
    pub runtime_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub work_dir: Option<String>,
    pub trigger_comment_id: Option<Uuid>,
    pub chat_session_id: Option<Uuid>,
    pub autopilot_run_id: Option<Uuid>,
    pub attempt: i32,
    pub max_attempts: i32,
    pub parent_task_id: Option<Uuid>,
    pub failure_reason: Option<String>,
    pub trigger_summary: Option<String>,
    pub force_fresh_session: bool,
    pub is_leader_task: bool,
    pub wait_reason: Option<String>,
    pub initiator_user_id: Option<Uuid>,
    pub handoff_note: Option<String>,
    pub prepare_lease_expires_at: Option<DateTime<Utc>>,
    pub team_id: Option<Uuid>,
    pub runtime_mcp_overlay: Option<serde_json::Value>,
    pub escalation_for_task_id: Option<Uuid>,
    pub fire_at: Option<DateTime<Utc>>,
    pub originator_user_id: Option<Uuid>,
    pub runtime_connected_apps: Option<serde_json::Value>,
    pub coalesced_comment_ids: Option<Vec<Uuid>>,
    pub delivered_comment_ids: Option<Vec<Uuid>>,
    pub chat_input_task_id: Option<Uuid>,
    pub chat_finalize_deferred_at: Option<DateTime<Utc>>,
    pub originator_source: Option<String>,
    pub delegated_from_task_id: Option<Uuid>,
    pub retry_of_task_id: Option<Uuid>,
    pub rerun_of_task_id: Option<Uuid>,
    pub rule_version_id: Option<Uuid>,
    pub trigger_evidence_kind: Option<String>,
    pub trigger_evidence_ref_id: Option<Uuid>,
    pub accountable_user_id: Option<Uuid>,
    pub session_rollout_missing: bool,
    pub retired_session_id: Option<String>,
    pub quick_actions_disabled: bool,
    pub regenerate_quick_actions_for: Option<Uuid>,
    pub branch_name: Option<String>,
    pub durable_work_dir: Option<String>,
    pub execution_lane_key: ExecutionLaneKey,
}

pub async fn cancel_deferred_escalations_for_issue_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Vec<CancelDeferredEscalationsForIssueAgentRow>> {
    let rows = sqlx::query(
        r#"WITH cancelled AS (
    UPDATE agent_task_queue fallback
    SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
    FROM agent_task_queue primary_task
    WHERE fallback.escalation_for_task_id = primary_task.id
      AND fallback.status IN ('deferred', 'queued', 'dispatched', 'waiting_local_directory')
      AND primary_task.issue_id = $1
      AND primary_task.agent_id = $2
    RETURNING fallback.id, fallback.agent_id, fallback.issue_id, fallback.status, fallback.priority, fallback.dispatched_at, fallback.started_at, fallback.completed_at, fallback.result, fallback.error, fallback.created_at, fallback.context, fallback.runtime_id, fallback.session_id, fallback.work_dir, fallback.trigger_comment_id, fallback.chat_session_id, fallback.autopilot_run_id, fallback.attempt, fallback.max_attempts, fallback.parent_task_id, fallback.failure_reason, fallback.trigger_summary, fallback.force_fresh_session, fallback.is_leader_task, fallback.wait_reason, fallback.initiator_user_id, fallback.handoff_note, fallback.prepare_lease_expires_at, fallback.team_id, fallback.runtime_mcp_overlay, fallback.escalation_for_task_id, fallback.fire_at, fallback.originator_user_id, fallback.runtime_connected_apps, fallback.coalesced_comment_ids, fallback.delivered_comment_ids, fallback.chat_input_task_id, fallback.chat_finalize_deferred_at, fallback.originator_source, fallback.delegated_from_task_id, fallback.retry_of_task_id, fallback.rerun_of_task_id, fallback.rule_version_id, fallback.trigger_evidence_kind, fallback.trigger_evidence_ref_id, fallback.accountable_user_id, fallback.session_rollout_missing, fallback.retired_session_id, fallback.quick_actions_disabled, fallback.regenerate_quick_actions_for, fallback.branch_name, fallback.durable_work_dir, fallback.execution_lane_key
)
SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM cancelled"#
    )
        .bind(issue_id)
        .bind(agent_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(CancelDeferredEscalationsForIssueAgentRow {
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_deferred_escalations_for_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    escalation_for_task_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE escalation_for_task_id = $1
  AND status IN ('deferred', 'queued', 'dispatched', 'waiting_local_directory')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(escalation_for_task_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_pending_tasks_by_issue_and_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE issue_id = $1 AND agent_id = $2
  AND status IN ('queued', 'dispatched', 'deferred')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(issue_id)
        .bind(agent_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_queued_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE id = $1
  AND chat_session_id = $2
  AND status = 'queued'
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .bind(chat_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn cancel_queued_agent_tasks_for_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH head AS MATERIALIZED (
  SELECT candidate.id
  FROM agent_task_queue AS candidate
  WHERE candidate.chat_session_id = $1
    AND candidate.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
    AND candidate.regenerate_quick_actions_for IS NULL
  ORDER BY
    CASE
      WHEN candidate.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
      WHEN candidate.status = 'deferred' THEN 1
      ELSE 2
    END,
    candidate.priority DESC,
    candidate.created_at ASC,
    candidate.id ASC
  LIMIT 1
)
UPDATE agent_task_queue AS queued
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE queued.chat_session_id = $1
  AND queued.status = 'queued'
  AND queued.id IS DISTINCT FROM (SELECT id FROM head)
RETURNING queued.id, queued.agent_id, queued.issue_id, queued.status, queued.priority, queued.dispatched_at, queued.started_at, queued.completed_at, queued.result, queued.error, queued.created_at, queued.context, queued.runtime_id, queued.session_id, queued.work_dir, queued.trigger_comment_id, queued.chat_session_id, queued.autopilot_run_id, queued.attempt, queued.max_attempts, queued.parent_task_id, queued.failure_reason, queued.trigger_summary, queued.force_fresh_session, queued.is_leader_task, queued.wait_reason, queued.initiator_user_id, queued.handoff_note, queued.prepare_lease_expires_at, queued.team_id, queued.runtime_mcp_overlay, queued.escalation_for_task_id, queued.fire_at, queued.originator_user_id, queued.runtime_connected_apps, queued.coalesced_comment_ids, queued.delivered_comment_ids, queued.chat_input_task_id, queued.chat_finalize_deferred_at, queued.originator_source, queued.delegated_from_task_id, queued.retry_of_task_id, queued.rerun_of_task_id, queued.rule_version_id, queued.trigger_evidence_kind, queued.trigger_evidence_ref_id, queued.accountable_user_id, queued.session_rollout_missing, queued.retired_session_id, queued.quick_actions_disabled, queued.regenerate_quick_actions_for, queued.branch_name, queued.durable_work_dir, execution_lane_key"#
    )
        .bind(chat_session_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn cancel_superseded_deferred_retries_for_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue r
SET status = 'cancelled', completed_at = now(), prepare_lease_expires_at = NULL
WHERE r.runtime_id = ANY($1::uuid[])
  AND r.status = 'deferred'
  AND r.issue_id IS NOT NULL
  AND r.retry_of_task_id IS NOT NULL
  AND r.escalation_for_task_id IS NULL
  AND COALESCE(r.context->>'channel_issue_media_pending', '') <> 'true'
  AND EXISTS (
    SELECT 1 FROM agent_task_queue successor
    WHERE successor.execution_lane_key = r.execution_lane_key
      AND successor.id <> r.id
      AND successor.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
  )
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(runtime_ids)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn claim_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    prepare_lease_secs: f64,
    agent_id: Uuid,
    runtime_id: Uuid,
    runtime_stale_secs: f64,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'dispatched',
    dispatched_at = now(),
    prepare_lease_expires_at = now() + make_interval(secs => $1::double precision)
WHERE id = (
    SELECT atq.id FROM agent_task_queue atq
    JOIN agent task_agent ON task_agent.id = atq.agent_id
    WHERE atq.agent_id = $2
      AND atq.runtime_id = $3
      AND atq.status = 'queued'
      AND task_agent.archived_at IS NULL
      AND EXISTS (
          SELECT 1 FROM agent_runtime r
          WHERE r.id = atq.runtime_id
            AND r.status = 'online'
            AND COALESCE(r.last_seen_at, r.updated_at) >=
              now() - make_interval(secs => $4::double precision)
      )
      AND (
          atq.issue_id IS NULL
          OR dependency_graph_issue_gate_open(
              (SELECT i.workspace_id FROM issue i WHERE i.id = atq.issue_id),
              atq.issue_id
          )
      )
      AND NOT EXISTS (
          SELECT 1 FROM agent_task_queue active
          WHERE active.execution_lane_key = atq.execution_lane_key
            AND active.status IN ('dispatched', 'running', 'waiting_local_directory')
      )
    ORDER BY atq.priority DESC, atq.created_at ASC, atq.id ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(prepare_lease_secs)
        .bind(agent_id)
        .bind(runtime_id)
        .bind(runtime_stale_secs)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn claim_chat_finalize_deferred(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET chat_finalize_deferred_at = NULL
WHERE id = $1 AND chat_finalize_deferred_at IS NOT NULL
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn clear_agent_composio_toolkit_allowlist(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET composio_toolkit_allowlist = NULL, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn clear_agent_mcp_config(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET mcp_config = NULL, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn clear_agent_service_tier(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET service_tier = NULL, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn clear_agent_thinking_level(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET thinking_level = NULL, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn complete_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    result: &serde_json::Value,
    session_id: Option<&str>,
    work_dir: Option<&str>,
    session_rollout_missing: bool,
    durable_work_dir: Option<&str>,
    branch_name: Option<&str>,
    retired_session_id: Option<&str>,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'completed', completed_at = now(), result = $2,
    session_id = CASE WHEN $5 THEN NULL ELSE $3 END,
    work_dir = $4,
    durable_work_dir = COALESCE($6, durable_work_dir),
    branch_name = COALESCE($7, branch_name),
    session_rollout_missing = $5,
    retired_session_id = COALESCE($8, retired_session_id),
    prepare_lease_expires_at = NULL
WHERE id = $1 AND status = 'running'
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .bind(result)
        .bind(session_id)
        .bind(work_dir)
        .bind(session_rollout_missing)
        .bind(durable_work_dir)
        .bind(branch_name)
        .bind(retired_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn count_delegated_failure_recovery_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    failed_task_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*)
FROM agent_task_queue
WHERE trigger_evidence_kind = 'delegated_failure'
  AND trigger_evidence_ref_id = $1"#,
    )
    .bind(failed_task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn count_running_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM agent_task_queue
WHERE agent_id = $1 AND status IN ('dispatched', 'running', 'waiting_local_directory')"#,
    )
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &str,
    avatar_url: Option<&str>,
    runtime_mode: &str,
    runtime_config: &serde_json::Value,
    runtime_id: Uuid,
    visibility: &str,
    max_concurrent_tasks: i32,
    owner_id: Uuid,
    instructions: &str,
    custom_env: &serde_json::Value,
    custom_args: &serde_json::Value,
    mcp_config: &serde_json::Value,
    model: Option<&str>,
    thinking_level: Option<&str>,
    service_tier: Option<&str>,
    composio_toolkit_allowlist: &[String],
    permission_mode: Option<&str>,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"INSERT INTO agent (
    workspace_id, name, description, avatar_url, runtime_mode,
    runtime_config, runtime_id, visibility, max_concurrent_tasks, owner_id,
    instructions, custom_env, custom_args, mcp_config, model, thinking_level,
    service_tier,
    composio_toolkit_allowlist, permission_mode
) VALUES (
    $1, $2, $3, $4, $5,
    $6, $7, $8, $9, $10,
    $11, $12, $13, $14, $15, $16,
    $17,
    $18::text[],
    COALESCE($19::text, 'private')
)
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(avatar_url)
        .bind(runtime_mode)
        .bind(runtime_config)
        .bind(runtime_id)
        .bind(visibility)
        .bind(max_concurrent_tasks)
        .bind(owner_id)
        .bind(instructions)
        .bind(custom_env)
        .bind(custom_args)
        .bind(mcp_config)
        .bind(model)
        .bind(thinking_level)
        .bind(service_tier)
        .bind(composio_toolkit_allowlist)
        .bind(permission_mode)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn create_agent_builder(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    runtime_mode: &str,
    runtime_id: Uuid,
    owner_id: Uuid,
    instructions: &str,
    model: Option<&str>,
    system_key: Option<&str>,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"INSERT INTO agent (
    workspace_id, name, description, runtime_mode, runtime_config, runtime_id,
    visibility, permission_mode, max_concurrent_tasks, owner_id, instructions,
    custom_env, custom_args, model, kind, system_key
) VALUES (
    $1, $2, '', $3, '{}'::jsonb, $4,
    'private', 'private', 1, $5, $6,
    '{}'::jsonb, '[]'::jsonb, $7, 'system', $8
)
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(runtime_mode)
        .bind(runtime_id)
        .bind(owner_id)
        .bind(instructions)
        .bind(model)
        .bind(system_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn create_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    runtime_id: Uuid,
    issue_id: Uuid,
    priority: i32,
    trigger_comment_id: Uuid,
    coalesced_comment_ids: Vec<Uuid>,
    trigger_summary: Option<&str>,
    force_fresh_session: Option<bool>,
    is_leader_task: Option<bool>,
    handoff_note: Option<&str>,
    team_id: Uuid,
    head_sha: Option<&str>,
    originator_user_id: Uuid,
    accountable_user_id: Uuid,
    runtime_mcp_overlay: &serde_json::Value,
    runtime_connected_apps: &serde_json::Value,
    originator_source: Option<&str>,
    delegated_from_task_id: Uuid,
    rule_version_id: Uuid,
    rerun_of_task_id: Uuid,
    trigger_evidence_kind: Option<&str>,
    trigger_evidence_ref_id: Uuid,
    id: Uuid,
    initial_context: &serde_json::Value,
    initial_status: &str,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, status, priority, trigger_comment_id,
    coalesced_comment_ids, trigger_summary, force_fresh_session, is_leader_task, handoff_note,
    team_id, context, originator_user_id, accountable_user_id, runtime_mcp_overlay, runtime_connected_apps,
    originator_source, delegated_from_task_id, rule_version_id, rerun_of_task_id, trigger_evidence_kind, trigger_evidence_ref_id,
    id
)
SELECT
    $1, $2, $3, CASE WHEN $25::text = 'deferred' THEN 'deferred' ELSE 'queued' END, $4,
    NULLIF($5::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE($6::uuid[], '{}'),
    $7,
    COALESCE($8::boolean, FALSE),
    COALESCE($9::boolean, FALSE),
    $10,
    NULLIF($11::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF(
        COALESCE(NULLIF($24::jsonb, 'null'::jsonb), '{}'::jsonb) ||
            CASE
                WHEN COALESCE($12::text, '') <> ''
                THEN jsonb_build_object('head_sha', $12::text)
                ELSE '{}'::jsonb
            END,
        '{}'::jsonb
    ),
    NULLIF($13::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($14::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $15,
    $16,
    $17,
    NULLIF($18::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($19::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($20::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $21,
    NULLIF($22::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE($23::uuid, gen_random_uuid())
WHERE lock_task_owner_rows($1, $3, $2)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(agent_id)
        .bind(runtime_id)
        .bind(issue_id)
        .bind(priority)
        .bind(trigger_comment_id)
        .bind(coalesced_comment_ids)
        .bind(trigger_summary)
        .bind(force_fresh_session)
        .bind(is_leader_task)
        .bind(handoff_note)
        .bind(team_id)
        .bind(head_sha)
        .bind(originator_user_id)
        .bind(accountable_user_id)
        .bind(runtime_mcp_overlay)
        .bind(runtime_connected_apps)
        .bind(originator_source)
        .bind(delegated_from_task_id)
        .bind(rule_version_id)
        .bind(rerun_of_task_id)
        .bind(trigger_evidence_kind)
        .bind(trigger_evidence_ref_id)
        .bind(id)
        .bind(initial_context)
        .bind(initial_status)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn merge_agent_task_context(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    context: &serde_json::Value,
) -> anyhow::Result<Option<serde_json::Value>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET context = COALESCE(context, '{}'::jsonb) || $2::jsonb
WHERE id = $1
RETURNING context"#,
    )
    .bind(task_id)
    .bind(context)
    .fetch_optional(executor)
    .await?;
    row.map(|value| value.try_get(0))
        .transpose()
        .map_err(Into::into)
}

pub async fn create_deferred_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    p_agent_id: Uuid,
    p_runtime_id: Uuid,
    p_issue_id: Uuid,
    agent_id: Uuid,
    runtime_id: Uuid,
    issue_id: Uuid,
    priority: i32,
    trigger_comment_id: Uuid,
    trigger_summary: Option<&str>,
    is_leader_task: Option<bool>,
    team_id: Uuid,
    escalation_for_task_id: Uuid,
    fire_at: Option<DateTime<Utc>>,
    originator_user_id: Uuid,
    accountable_user_id: Uuid,
    originator_source: Option<&str>,
    delegated_from_task_id: Uuid,
    trigger_evidence_kind: Option<&str>,
    trigger_evidence_ref_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, status, priority, trigger_comment_id,
    trigger_summary, is_leader_task, team_id, escalation_for_task_id, fire_at,
    originator_user_id, accountable_user_id, originator_source,
    delegated_from_task_id, trigger_evidence_kind, trigger_evidence_ref_id,
    id
)
SELECT
    $4, $5, $6, 'deferred', $7,
    NULLIF($8::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $9,
    COALESCE($10::boolean, FALSE),
    NULLIF($11::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $12,
    $13,
    NULLIF($14::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($15::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $16,
    NULLIF($17::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $18,
    NULLIF($19::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE($20::uuid, gen_random_uuid())
WHERE lock_task_owner_rows($1, $3, $2)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(p_agent_id)
        .bind(p_runtime_id)
        .bind(p_issue_id)
        .bind(agent_id)
        .bind(runtime_id)
        .bind(issue_id)
        .bind(priority)
        .bind(trigger_comment_id)
        .bind(trigger_summary)
        .bind(is_leader_task)
        .bind(team_id)
        .bind(escalation_for_task_id)
        .bind(fire_at)
        .bind(originator_user_id)
        .bind(accountable_user_id)
        .bind(originator_source)
        .bind(delegated_from_task_id)
        .bind(trigger_evidence_kind)
        .bind(trigger_evidence_ref_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn create_deferred_channel_issue_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    runtime_id: Uuid,
    issue_id: Uuid,
    priority: i32,
    trigger_comment_id: Uuid,
    coalesced_comment_ids: Vec<Uuid>,
    trigger_summary: Option<&str>,
    force_fresh_session: Option<bool>,
    is_leader_task: Option<bool>,
    handoff_note: Option<&str>,
    team_id: Uuid,
    head_sha: Option<&str>,
    originator_user_id: Uuid,
    accountable_user_id: Uuid,
    runtime_mcp_overlay: &serde_json::Value,
    runtime_connected_apps: &serde_json::Value,
    originator_source: Option<&str>,
    delegated_from_task_id: Uuid,
    rule_version_id: Uuid,
    rerun_of_task_id: Uuid,
    trigger_evidence_kind: Option<&str>,
    trigger_evidence_ref_id: Uuid,
    fire_at: Option<DateTime<Utc>>,
    id: Uuid,
    initial_context: &serde_json::Value,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, status, priority, trigger_comment_id,
    coalesced_comment_ids, trigger_summary, force_fresh_session, is_leader_task, handoff_note,
    team_id, context, originator_user_id, accountable_user_id, runtime_mcp_overlay, runtime_connected_apps,
    originator_source, delegated_from_task_id, rule_version_id, rerun_of_task_id,
    trigger_evidence_kind, trigger_evidence_ref_id, fire_at,
    id
)
SELECT
    $1, $2, $3, 'deferred', $4,
    NULLIF($5::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    COALESCE($6::uuid[], '{}'),
    $7,
    COALESCE($8::boolean, FALSE),
    COALESCE($9::boolean, FALSE),
    $10,
    NULLIF($11::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    jsonb_strip_nulls(
        COALESCE(NULLIF($25::jsonb, 'null'::jsonb), '{}'::jsonb) ||
        jsonb_build_object(
            'head_sha', NULLIF(COALESCE($12::text, ''), ''),
            'channel_issue_media_pending', TRUE
        )
    ),
    NULLIF($13::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($14::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $15,
    $16,
    $17,
    NULLIF($18::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($19::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    NULLIF($20::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $21,
    NULLIF($22::uuid, '00000000-0000-0000-0000-000000000000'::uuid),
    $23,
    COALESCE($24::uuid, gen_random_uuid())
WHERE lock_task_owner_rows($1, $3, $2)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(agent_id)
        .bind(runtime_id)
        .bind(issue_id)
        .bind(priority)
        .bind(trigger_comment_id)
        .bind(coalesced_comment_ids)
        .bind(trigger_summary)
        .bind(force_fresh_session)
        .bind(is_leader_task)
        .bind(handoff_note)
        .bind(team_id)
        .bind(head_sha)
        .bind(originator_user_id)
        .bind(accountable_user_id)
        .bind(runtime_mcp_overlay)
        .bind(runtime_connected_apps)
        .bind(originator_source)
        .bind(delegated_from_task_id)
        .bind(rule_version_id)
        .bind(rerun_of_task_id)
        .bind(trigger_evidence_kind)
        .bind(trigger_evidence_ref_id)
        .bind(fire_at)
        .bind(id)
        .bind(initial_context)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn create_quick_create_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    runtime_id: Uuid,
    priority: i32,
    context: &serde_json::Value,
    originator_user_id: Uuid,
    accountable_user_id: Uuid,
    runtime_mcp_overlay: &serde_json::Value,
    runtime_connected_apps: &serde_json::Value,
    originator_source: Option<&str>,
    trigger_evidence_kind: Option<&str>,
    trigger_evidence_ref_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, status, priority, context, originator_user_id,
    accountable_user_id, runtime_mcp_overlay, runtime_connected_apps,
    originator_source, trigger_evidence_kind, trigger_evidence_ref_id,
    id
)
SELECT
    $1, $2, NULL, 'queued', $3, $4,
    $5,
    $6,
    $7,
    $8,
    $9,
    $10,
    $11,
    COALESCE($12::uuid, gen_random_uuid())
WHERE lock_task_owner_rows($1, NULL, $2)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(agent_id)
        .bind(runtime_id)
        .bind(priority)
        .bind(context)
        .bind(originator_user_id)
        .bind(accountable_user_id)
        .bind(runtime_mcp_overlay)
        .bind(runtime_connected_apps)
        .bind(originator_source)
        .bind(trigger_evidence_kind)
        .bind(trigger_evidence_ref_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn create_retry_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    fire_at: Option<DateTime<Utc>>,
    max_attempts: Option<i32>,
    runtime_mcp_overlay: &serde_json::Value,
    runtime_connected_apps: &serde_json::Value,
    new_task_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, chat_session_id, autopilot_run_id,
    status, priority, trigger_comment_id, coalesced_comment_ids, trigger_summary, context,
    session_id, work_dir,
    attempt, max_attempts, parent_task_id, force_fresh_session, is_leader_task,
    team_id, originator_user_id, accountable_user_id, runtime_mcp_overlay, runtime_connected_apps,
    originator_source, delegated_from_task_id, rule_version_id,
    trigger_evidence_kind, trigger_evidence_ref_id, retry_of_task_id,
    chat_input_task_id, fire_at,
    id
)
SELECT
    p.agent_id, p.runtime_id, p.issue_id, p.chat_session_id, p.autopilot_run_id,
    CASE WHEN $2::timestamptz IS NOT NULL THEN 'deferred' ELSE 'queued' END,
    CASE WHEN p.chat_session_id IS NOT NULL THEN GREATEST(p.priority, 3) ELSE p.priority END,
    p.trigger_comment_id, p.coalesced_comment_ids, p.trigger_summary, p.context,
    CASE WHEN p.failure_reason IS NOT DISTINCT FROM 'codex_semantic_inactivity' THEN NULL ELSE p.session_id END,
    CASE WHEN p.failure_reason IS NOT DISTINCT FROM 'codex_semantic_inactivity' THEN NULL ELSE p.work_dir END,
    p.attempt + 1, COALESCE($3::int, p.max_attempts), p.id,
    p.failure_reason IS NOT DISTINCT FROM 'codex_semantic_inactivity',
    p.is_leader_task,
    p.team_id,
    p.originator_user_id,
    p.accountable_user_id,
    $4,
    $5,
    p.originator_source, p.delegated_from_task_id, p.rule_version_id,
    p.trigger_evidence_kind, p.trigger_evidence_ref_id, p.id,
    p.chat_input_task_id, $2,
    -- Named new_task_id, not id: $1 above is the PARENT task's id.
    COALESCE($6::uuid, gen_random_uuid())
FROM agent_task_queue p
WHERE p.id = $1
  AND lock_task_owner_rows(p.agent_id, p.issue_id, p.runtime_id)
ON CONFLICT (issue_id, agent_id) WHERE (
           status IN ('queued', 'dispatched')
           AND COALESCE(context->>'side_chat_parent_task_id', '') = ''
       )
       OR (status = 'deferred' AND context->>'channel_issue_media_pending' = 'true')
DO NOTHING
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .bind(fire_at)
        .bind(max_attempts)
        .bind(runtime_mcp_overlay)
        .bind(runtime_connected_apps)
        .bind(new_task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn create_system_user_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &str,
    avatar_url: Option<&str>,
    runtime_mode: &str,
    runtime_id: Uuid,
    model: Option<&str>,
    visibility: &str,
    permission_mode: &str,
    max_concurrent_tasks: i32,
    owner_id: Uuid,
    system_key: Option<&str>,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"INSERT INTO agent (
    workspace_id, name, description, avatar_url, runtime_mode, runtime_config,
    runtime_id, model, visibility, permission_mode, max_concurrent_tasks,
    owner_id, instructions, custom_env, custom_args, kind, system_key
) VALUES (
    $1, $2, $3, $4, $5, '{}'::jsonb,
    $6, $7, $8, $9, $10,
    $11, '', '{}'::jsonb, '[]'::jsonb, 'user', $12
)
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(avatar_url)
        .bind(runtime_mode)
        .bind(runtime_id)
        .bind(model)
        .bind(visibility)
        .bind(permission_mode)
        .bind(max_concurrent_tasks)
        .bind(owner_id)
        .bind(system_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn delete_system_agent_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH victim AS MATERIALIZED (
    SELECT id FROM agent
    WHERE id = $1 AND kind = 'system' AND system_key LIKE 'agent_builder:%'
), revoked_grants AS (
    UPDATE authorization_grant
    SET revoked_at = COALESCE(revoked_at, now()), updated_at = now()
    WHERE revoked_at IS NULL
      AND (
          (resource_type = 'agent_definition' AND resource_id IN (SELECT id FROM victim))
          OR (principal_type = 'agent_definition' AND principal_id IN (SELECT id FROM victim))
      )
)
DELETE FROM agent WHERE id IN (SELECT id FROM victim)"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn expire_stale_queued_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    stale_before: chrono::DateTime<chrono::Utc>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH victims AS (
    SELECT id FROM agent_task_queue
    WHERE status = 'queued'
      AND created_at < $1
      AND NOT EXISTS (
          SELECT 1 FROM agent_task_queue retry_parent
          WHERE retry_parent.id = agent_task_queue.parent_task_id
            AND retry_parent.failure_reason = 'runtime_offline'
      )
    ORDER BY created_at ASC
    LIMIT $2::int
    FOR UPDATE SKIP LOCKED
)
UPDATE agent_task_queue t
SET status = 'failed',
    completed_at = now(),
    error = 'task expired in queue',
    failure_reason = 'queued_expired',
    prepare_lease_expires_at = NULL
FROM victims v
WHERE t.id = v.id
  AND t.status = 'queued'
  AND t.created_at < $1
  AND NOT EXISTS (
      SELECT 1 FROM agent_task_queue retry_parent
      WHERE retry_parent.id = t.parent_task_id
        AND retry_parent.failure_reason = 'runtime_offline'
  )
RETURNING t.id, t.agent_id, t.issue_id, t.status, t.priority, t.dispatched_at, t.started_at, t.completed_at, t.result, t.error, t.created_at, t.context, t.runtime_id, t.session_id, t.work_dir, t.trigger_comment_id, t.chat_session_id, t.autopilot_run_id, t.attempt, t.max_attempts, t.parent_task_id, t.failure_reason, t.trigger_summary, t.force_fresh_session, t.is_leader_task, t.wait_reason, t.initiator_user_id, t.handoff_note, t.prepare_lease_expires_at, t.team_id, t.runtime_mcp_overlay, t.escalation_for_task_id, t.fire_at, t.originator_user_id, t.runtime_connected_apps, t.coalesced_comment_ids, t.delivered_comment_ids, t.chat_input_task_id, t.chat_finalize_deferred_at, t.originator_source, t.delegated_from_task_id, t.retry_of_task_id, t.rerun_of_task_id, t.rule_version_id, t.trigger_evidence_kind, t.trigger_evidence_ref_id, t.accountable_user_id, t.session_rollout_missing, t.retired_session_id, t.quick_actions_disabled, t.regenerate_quick_actions_for, t.branch_name, t.durable_work_dir, execution_lane_key"#
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn extend_agent_task_prepare_lease(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    runtime_id: Uuid,
    lease_secs: f64,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET prepare_lease_expires_at = now() + make_interval(secs => $3::double precision)
WHERE id = $1
  AND runtime_id = $2
  AND status IN ('dispatched', 'waiting_local_directory')
  AND started_at IS NULL
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .bind(runtime_id)
        .bind(lease_secs)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn fail_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    error: Option<&str>,
    failure_reason: Option<&str>,
    session_rollout_missing: bool,
    session_id: Option<&str>,
    work_dir: Option<&str>,
    durable_work_dir: Option<&str>,
    branch_name: Option<&str>,
    retired_session_id: Option<&str>,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'failed',
    completed_at = now(),
    error = $2,
    failure_reason = COALESCE($3, 'agent_error'),
    session_id = CASE WHEN $4 THEN NULL ELSE COALESCE($5, session_id) END,
    work_dir = COALESCE($6, work_dir),
    durable_work_dir = COALESCE($7, durable_work_dir),
    branch_name = COALESCE($8, branch_name),
    session_rollout_missing = $4,
    retired_session_id = COALESCE($9, retired_session_id),
    prepare_lease_expires_at = NULL
WHERE id = $1 AND status IN ('dispatched', 'running', 'waiting_local_directory')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .bind(error)
        .bind(failure_reason)
        .bind(session_rollout_missing)
        .bind(session_id)
        .bind(work_dir)
        .bind(durable_work_dir)
        .bind(branch_name)
        .bind(retired_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn fail_expired_runtime_reconnect_retries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    retry_before: chrono::DateTime<chrono::Utc>,
    runtime_fresh_after: chrono::DateTime<chrono::Utc>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH victims AS (
    SELECT retry.id
    FROM agent_task_queue retry
    JOIN agent_task_queue parent ON parent.id = retry.parent_task_id
    WHERE retry.status = 'deferred'
      AND retry.fire_at < $1
      AND parent.failure_reason = 'runtime_offline'
      AND NOT EXISTS (
          SELECT 1 FROM agent_runtime runtime
          WHERE runtime.id = retry.runtime_id
            AND runtime.status = 'online'
            AND COALESCE(runtime.last_seen_at, runtime.updated_at) >= $2
      )
    ORDER BY retry.fire_at, retry.created_at
    LIMIT $3::int
    FOR UPDATE OF retry SKIP LOCKED
)
UPDATE agent_task_queue AS retry
SET status = 'failed',
    completed_at = now(),
    error = 'runtime did not reconnect within the configured grace period',
    failure_reason = 'runtime_reconnect_timeout',
    wait_reason = NULL,
    prepare_lease_expires_at = NULL
FROM victims
WHERE retry.id = victims.id
  AND retry.status = 'deferred'
  AND retry.fire_at < $1
  AND EXISTS (
      SELECT 1 FROM agent_task_queue parent
      WHERE parent.id = retry.parent_task_id
        AND parent.failure_reason = 'runtime_offline'
  )
  AND NOT EXISTS (
      SELECT 1 FROM agent_runtime runtime
      WHERE runtime.id = retry.runtime_id
        AND runtime.status = 'online'
        AND COALESCE(runtime.last_seen_at, runtime.updated_at) >= $2
  )
RETURNING retry.id, retry.agent_id, retry.issue_id, retry.status, retry.priority, retry.dispatched_at, retry.started_at, retry.completed_at, retry.result, retry.error, retry.created_at, retry.context, retry.runtime_id, retry.session_id, retry.work_dir, retry.trigger_comment_id, retry.chat_session_id, retry.autopilot_run_id, retry.attempt, retry.max_attempts, retry.parent_task_id, retry.failure_reason, retry.trigger_summary, retry.force_fresh_session, retry.is_leader_task, retry.wait_reason, retry.initiator_user_id, retry.handoff_note, retry.prepare_lease_expires_at, retry.team_id, retry.runtime_mcp_overlay, retry.escalation_for_task_id, retry.fire_at, retry.originator_user_id, retry.runtime_connected_apps, retry.coalesced_comment_ids, retry.delivered_comment_ids, retry.chat_input_task_id, retry.chat_finalize_deferred_at, retry.originator_source, retry.delegated_from_task_id, retry.retry_of_task_id, retry.rerun_of_task_id, retry.rule_version_id, retry.trigger_evidence_kind, retry.trigger_evidence_ref_id, retry.accountable_user_id, retry.session_rollout_missing, retry.retired_session_id, retry.quick_actions_disabled, retry.regenerate_quick_actions_for, retry.branch_name, retry.durable_work_dir, execution_lane_key"#
    )
        .bind(retry_before)
        .bind(runtime_fresh_after)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn fail_stale_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    dispatch_before: chrono::DateTime<chrono::Utc>,
    lease_expired_before: chrono::DateTime<chrono::Utc>,
    runtime_fresh_after: chrono::DateTime<chrono::Utc>,
    runtime_abandoned_before: chrono::DateTime<chrono::Utc>,
    running_before: chrono::DateTime<chrono::Utc>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH victims AS (
SELECT task.id
FROM agent_task_queue task
WHERE (
    task.status = 'dispatched'
    AND task.dispatched_at < $1
    AND (task.prepare_lease_expires_at IS NULL OR task.prepare_lease_expires_at < $2)
    AND (
      task.runtime_id IS NULL
      OR NOT EXISTS (SELECT 1 FROM agent_runtime r WHERE r.id = task.runtime_id)
      OR EXISTS (
        SELECT 1 FROM agent_runtime r
        WHERE r.id = task.runtime_id
          AND ((r.status = 'online' AND COALESCE(r.last_seen_at, r.updated_at) >= $3)
               OR COALESCE(r.last_seen_at, r.updated_at) < $4)
      )
    )
  ) OR (
    task.status = 'running'
    AND task.started_at < $5
    AND (
      task.runtime_id IS NULL
      OR NOT EXISTS (
        SELECT 1 FROM agent_runtime r
        WHERE r.id = task.runtime_id
          AND COALESCE(r.last_seen_at, r.updated_at) >= $4
      )
    )
  )
ORDER BY COALESCE(task.dispatched_at, task.started_at), task.id
LIMIT $6::int
FOR UPDATE OF task SKIP LOCKED
)
UPDATE agent_task_queue task
SET status = 'failed', completed_at = now(), error = 'task timed out',
    failure_reason = 'timeout',
    prepare_lease_expires_at = NULL
FROM victims
WHERE task.id = victims.id
RETURNING task.id, task.agent_id, task.issue_id, task.status, task.priority, task.dispatched_at, task.started_at, task.completed_at, task.result, task.error, task.created_at, task.context, task.runtime_id, task.session_id, task.work_dir, task.trigger_comment_id, task.chat_session_id, task.autopilot_run_id, task.attempt, task.max_attempts, task.parent_task_id, task.failure_reason, task.trigger_summary, task.force_fresh_session, task.is_leader_task, task.wait_reason, task.initiator_user_id, task.handoff_note, task.prepare_lease_expires_at, task.team_id, task.runtime_mcp_overlay, task.escalation_for_task_id, task.fire_at, task.originator_user_id, task.runtime_connected_apps, task.coalesced_comment_ids, task.delivered_comment_ids, task.chat_input_task_id, task.chat_finalize_deferred_at, task.originator_source, task.delegated_from_task_id, task.retry_of_task_id, task.rerun_of_task_id, task.rule_version_id, task.trigger_evidence_kind, task.trigger_evidence_ref_id, task.accountable_user_id, task.session_rollout_missing, task.retired_session_id, task.quick_actions_disabled, task.regenerate_quick_actions_for, task.branch_name, task.durable_work_dir, execution_lane_key"#
    )
        .bind(dispatch_before)
        .bind(lease_expired_before)
        .bind(runtime_fresh_after)
        .bind(runtime_abandoned_before)
        .bind(running_before)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn get_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn get_agent_by_system_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    system_key: Option<&str>,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE workspace_id = $1 AND system_key = $2 AND archived_at IS NULL
ORDER BY created_at ASC, id ASC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(system_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn get_agent_for_claim_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE id = $1
FOR UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn get_agent_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE id = $1
FOR UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn get_agent_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE id = $1 AND workspace_id = $2 AND kind = 'user'"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn get_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn get_agent_task_for_delegated_failure_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE id = $1
FOR UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn get_agent_task_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"SELECT atq.id, atq.agent_id, atq.issue_id, atq.status, atq.priority, atq.dispatched_at, atq.started_at, atq.completed_at, atq.result, atq.error, atq.created_at, atq.context, atq.runtime_id, atq.session_id, atq.work_dir, atq.trigger_comment_id, atq.chat_session_id, atq.autopilot_run_id, atq.attempt, atq.max_attempts, atq.parent_task_id, atq.failure_reason, atq.trigger_summary, atq.force_fresh_session, atq.is_leader_task, atq.wait_reason, atq.initiator_user_id, atq.handoff_note, atq.prepare_lease_expires_at, atq.team_id, atq.runtime_mcp_overlay, atq.escalation_for_task_id, atq.fire_at, atq.originator_user_id, atq.runtime_connected_apps, atq.coalesced_comment_ids, atq.delivered_comment_ids, atq.chat_input_task_id, atq.chat_finalize_deferred_at, atq.originator_source, atq.delegated_from_task_id, atq.retry_of_task_id, atq.rerun_of_task_id, atq.rule_version_id, atq.trigger_evidence_kind, atq.trigger_evidence_ref_id, atq.accountable_user_id, atq.session_rollout_missing, atq.retired_session_id, atq.quick_actions_disabled, atq.regenerate_quick_actions_for, atq.branch_name, atq.durable_work_dir, execution_lane_key FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
WHERE atq.id = $1 AND a.workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetLastTaskSessionRow {
    pub session_id: Option<String>,
    pub work_dir: Option<String>,
    pub runtime_id: Option<Uuid>,
}

pub async fn get_last_task_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<GetLastTaskSessionRow>> {
    let row = sqlx::query(
        r#"WITH retired_sessions AS (
    SELECT DISTINCT r.retired_session_id AS session_id
    FROM agent_task_queue r
    WHERE r.agent_id = $1 AND r.issue_id = $2
      AND r.retired_session_id IS NOT NULL
      AND COALESCE(r.context->>'side_chat_parent_task_id', '') = ''
), resume_overflow_at AS (
    SELECT MAX(COALESCE(t.completed_at, t.started_at, t.dispatched_at, t.created_at)) AS at
    FROM agent_task_queue t
    WHERE t.agent_id = $1 AND t.issue_id = $2
      AND t.status = 'failed'
      AND COALESCE(t.context->>'side_chat_parent_task_id', '') = ''
      AND (
        COALESCE(t.failure_reason, '') = 'codex_resume_oversized'
        OR (COALESCE(t.error, '') ILIKE '%thread/resume failed%' AND COALESCE(t.error, '') ILIKE '%token too long%')
      )
), latest_per_session AS (
    SELECT DISTINCT ON (t.session_id)
        t.session_id, t.work_dir, t.runtime_id, t.status, t.failure_reason, t.error,
        COALESCE(t.completed_at, t.started_at, t.dispatched_at, t.created_at) AS terminal_at
    FROM agent_task_queue t
    WHERE t.agent_id = $1 AND t.issue_id = $2
      AND t.session_id IS NOT NULL
      AND t.status IN ('completed', 'failed', 'cancelled')
      AND COALESCE(t.context->>'side_chat_parent_task_id', '') = ''
    ORDER BY t.session_id, COALESCE(t.completed_at, t.started_at, t.dispatched_at, t.created_at) DESC
)
SELECT session_id, work_dir, runtime_id FROM latest_per_session
WHERE session_id NOT IN (SELECT session_id FROM retired_sessions)
  AND (
    status IN ('completed', 'cancelled')
    OR (
      status = 'failed'
      AND COALESCE(failure_reason, '') NOT IN ('iteration_limit', 'agent_fallback_message', 'api_invalid_request', 'codex_semantic_inactivity', 'agent_error.context_overflow', 'codex_resume_oversized')
      AND NOT (COALESCE(error, '') ILIKE '%400%' AND COALESCE(error, '') ILIKE '%invalid_request_error%')
      AND NOT (COALESCE(error, '') ILIKE '%image dimensions exceed max allowed size%' AND COALESCE(error, '') ILIKE '%image.source.base64.data%')
      -- A provider credential-resolution failure ("Could not resolve
      -- authentication method...") is deterministic on resume: the missing
      -- api_key / auth_token / header is baked into the session's provider
      -- state, so replaying it reproduces the same provider error forever. It
      -- is classified agent_error.unknown (resume-safe), so this text guard is
      -- the only thing that keeps a wedged issue from resuming the same dead
      -- session on its next trigger — there is no daemon upgrade to wait for.
      -- Keep in sync with ResumeUnsafeFailure and GetLastChatTaskSession.
      -- The phrase itself lives in taskfailure.AuthMethodUnresolved, which the
      -- daemon's in-turn fresh-session retry reads (GH #6777). This guard stays
      -- because it is the only protection for rows an older daemon wrote.
      AND NOT (COALESCE(error, '') ILIKE '%could not resolve authentication method%')
      AND NOT (COALESCE(error, '') ~* 'must not be empty|must be non-?empty|must have non-?empty|non-?empty content|cannot be empty|should not be empty'
               AND COALESCE(error, '') ~* 'role[^a-z0-9]{0,2}assistant|assistant message|message at position|messages\.[0-9]|messages\[[0-9]')
    )
  )
  -- PB-5722: a resume that overflowed the reader names no session, so it can
  -- only be excluded by time, not by matching the failed row. Drop every
  -- session whose last terminal activity predates the newest such failure: one
  -- of them IS the oversized thread, and the row that would tell us which is
  -- exactly the row that could not be written. Anything that terminated after
  -- the overflow is the fresh thread that replaced it, so this un-blocks itself
  -- as soon as one succeeds.
  AND (
    (SELECT at FROM resume_overflow_at) IS NULL
    OR terminal_at > (SELECT at FROM resume_overflow_at)
  )
ORDER BY terminal_at DESC
LIMIT 1"#
    )
        .bind(agent_id)
        .bind(issue_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetLastTaskSessionRow {
        session_id: row.try_get(0)?,
        work_dir: row.try_get(1)?,
        runtime_id: row.try_get(2)?,
    }))
}

pub async fn get_last_task_started_at_for_issue_and_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<Option<DateTime<Utc>>>> {
    let row = sqlx::query(
        r#"SELECT started_at FROM agent_task_queue
WHERE agent_id = $1 AND issue_id = $2 AND started_at IS NOT NULL
  AND COALESCE(context->>'side_chat_parent_task_id', '') = ''
ORDER BY started_at DESC
LIMIT 1"#,
    )
    .bind(agent_id)
    .bind(issue_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn get_latest_chat_task_rollout_missing(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT COALESCE(session_rollout_missing, FALSE) FROM agent_task_queue
WHERE chat_session_id = $1
  AND status IN ('completed', 'failed')
  AND started_at IS NOT NULL
ORDER BY COALESCE(completed_at, started_at, dispatched_at, created_at) DESC
LIMIT 1"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetLatestTaskRoleForIssueAndAgentRow {
    pub is_leader_task: bool,
    pub team_id: Option<Uuid>,
}

pub async fn get_latest_task_role_for_issue_and_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<GetLatestTaskRoleForIssueAndAgentRow>> {
    let row = sqlx::query(
        r#"SELECT is_leader_task, team_id FROM agent_task_queue
WHERE issue_id = $1 AND agent_id = $2
  AND COALESCE(context->>'side_chat_parent_task_id', '') = ''
ORDER BY created_at DESC
LIMIT 1"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetLatestTaskRoleForIssueAndAgentRow {
        is_leader_task: row.try_get(0)?,
        team_id: row.try_get(1)?,
    }))
}

pub async fn get_latest_task_rollout_missing(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT COALESCE(session_rollout_missing, FALSE) FROM agent_task_queue
WHERE agent_id = $1 AND issue_id = $2
  AND status IN ('completed', 'failed')
  AND started_at IS NOT NULL
  AND COALESCE(context->>'side_chat_parent_task_id', '') = ''
ORDER BY COALESCE(completed_at, started_at, dispatched_at, created_at) DESC
LIMIT 1"#,
    )
    .bind(agent_id)
    .bind(issue_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetWorkspaceAgentActivity30dRow {
    pub agent_id: Option<Uuid>,
    pub bucket: Option<DateTime<Utc>>,
    pub task_count: i32,
    pub failed_count: i32,
}

pub async fn get_workspace_agent_activity30d(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<GetWorkspaceAgentActivity30dRow>> {
    let rows = sqlx::query(
        r#"SELECT
    atq.agent_id,
    DATE_TRUNC('day', atq.completed_at)::timestamptz AS bucket,
    COUNT(*)::int AS task_count,
    COUNT(*) FILTER (WHERE atq.status = 'failed')::int AS failed_count
FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
WHERE a.workspace_id = $1
  AND atq.completed_at IS NOT NULL
  AND atq.completed_at > now() - INTERVAL '30 days'
GROUP BY atq.agent_id, bucket
ORDER BY atq.agent_id, bucket"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetWorkspaceAgentActivity30dRow {
            agent_id: row.try_get(0)?,
            bucket: row.try_get(1)?,
            task_count: row.try_get(2)?,
            failed_count: row.try_get(3)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetWorkspaceAgentRunCountsRow {
    pub agent_id: Option<Uuid>,
    pub run_count: i32,
}

pub async fn get_workspace_agent_run_counts(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<GetWorkspaceAgentRunCountsRow>> {
    let rows = sqlx::query(
        r#"SELECT
    atq.agent_id,
    COUNT(*)::int AS run_count
FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
WHERE a.workspace_id = $1
  AND atq.created_at > now() - INTERVAL '30 days'
GROUP BY atq.agent_id"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetWorkspaceAgentRunCountsRow {
            agent_id: row.try_get(0)?,
            run_count: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn has_active_task_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS has_active FROM agent_task_queue
WHERE issue_id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')"#,
    )
    .bind(issue_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_active_task_for_issue_and_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS has_active FROM agent_task_queue
WHERE issue_id = $1 AND agent_id = $2
  AND (
    status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
    OR (status = 'deferred' AND context->>'channel_issue_media_pending' = 'true')
  )"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_pending_task_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS has_pending FROM agent_task_queue
WHERE issue_id = $1 AND status IN ('queued', 'dispatched')"#,
    )
    .bind(issue_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_pending_task_for_issue_and_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
    head_sha: Option<&str>,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS has_pending FROM agent_task_queue
WHERE issue_id = $1 AND agent_id = $2
  AND (
    status IN ('queued', 'dispatched')
    OR (status = 'deferred' AND context->>'channel_issue_media_pending' = 'true')
  )
  AND (
    COALESCE($3::text, '') = ''
    OR context->>'head_sha' = $3::text
  )"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .bind(head_sha)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_pending_task_for_issue_and_agent_excluding_trigger_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
    exclude_trigger_comment_id: Uuid,
    head_sha: Option<&str>,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS has_pending FROM agent_task_queue
WHERE issue_id = $1
  AND agent_id = $2
  AND (
    status IN ('queued', 'dispatched')
    OR (status = 'deferred' AND context->>'channel_issue_media_pending' = 'true')
  )
  AND trigger_comment_id IS DISTINCT FROM $3::uuid
  AND (
    COALESCE($4::text, '') = ''
    OR context->>'head_sha' = $4::text
  )"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .bind(exclude_trigger_comment_id)
    .bind(head_sha)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_retry_task_for_parent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    parent_task_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0
FROM agent_task_queue
WHERE parent_task_id = $1
  AND status <> 'cancelled'"#,
    )
    .bind(parent_task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_task_covering_delegated_failure_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
    comment_id: Uuid,
    exclude_task_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS covered
FROM agent_task_queue
WHERE issue_id = $1
  AND agent_id = $2
  AND (
      $3::uuid = ANY(delivered_comment_ids)
      OR (
          id IS DISTINCT FROM $4::uuid
          AND (
              status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
              OR (status = 'deferred' AND context->>'channel_issue_media_pending' = 'true')
          )
          AND (trigger_comment_id = $3::uuid OR $3::uuid = ANY(coalesced_comment_ids))
      )
  )"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .bind(comment_id)
    .bind(exclude_task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn link_task_to_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET issue_id = $2
WHERE id = $1 AND issue_id IS NULL
  AND lock_task_owner_rows(NULL, $2, NULL)"#,
    )
    .bind(id)
    .bind(issue_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_active_agents_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE runtime_id = $1 AND archived_at IS NULL AND kind = 'user'
ORDER BY name ASC"#
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

pub async fn list_active_agents_by_runtime_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE runtime_id = $1 AND archived_at IS NULL AND kind = 'user'
ORDER BY name ASC
FOR UPDATE"#
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListActiveSiblingIssueTasksRow {
    pub task_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub issue_prefix: String,
    pub issue_number: i32,
    pub issue_title: String,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
}

pub async fn list_active_sibling_issue_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    task_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListActiveSiblingIssueTasksRow>> {
    let rows = sqlx::query(
        r#"SELECT
    atq.id AS task_id,
    i.id AS issue_id,
    w.issue_prefix,
    i.number AS issue_number,
    i.title AS issue_title,
    atq.status,
    atq.created_at,
    atq.started_at
FROM agent_task_queue atq
JOIN issue i ON i.id = atq.issue_id
JOIN workspace w ON w.id = i.workspace_id
WHERE atq.agent_id = $1
  AND atq.id <> $2
  AND i.workspace_id = $3
  AND atq.status IN ('dispatched', 'running', 'waiting_local_directory')
ORDER BY
    CASE atq.status
        WHEN 'running' THEN 0
        WHEN 'waiting_local_directory' THEN 1
        ELSE 2
    END,
    atq.created_at DESC
LIMIT 5"#,
    )
    .bind(agent_id)
    .bind(task_id)
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListActiveSiblingIssueTasksRow {
            task_id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            issue_prefix: row.try_get(2)?,
            issue_number: row.try_get(3)?,
            issue_title: row.try_get(4)?,
            status: row.try_get(5)?,
            created_at: row.try_get(6)?,
            started_at: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn list_active_tasks_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE issue_id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
ORDER BY created_at DESC"#
    )
        .bind(issue_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_agent_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE agent_id = $1
ORDER BY created_at DESC"#
    )
        .bind(agent_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE workspace_id = $1 AND archived_at IS NULL AND kind = 'user'
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
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

pub async fn list_all_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE workspace_id = $1 AND kind = 'user'
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
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

pub async fn list_all_agents_any_kind(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE workspace_id = $1
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
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

pub async fn list_chat_finalize_deferred_expired(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    deferred_before: DateTime<Utc>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE chat_finalize_deferred_at IS NOT NULL
  AND chat_finalize_deferred_at < $1
ORDER BY chat_finalize_deferred_at
LIMIT $2::int"#
    )
        .bind(deferred_before)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_pending_delegated_failure_recoveries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    max_per_tick: i32,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"SELECT recovery.id, recovery.issue_id, recovery.author_type, recovery.author_id, recovery.content, recovery.type, recovery.created_at, recovery.updated_at, recovery.parent_id, recovery.workspace_id, recovery.resolved_at, recovery.resolved_by_type, recovery.resolved_by_id, recovery.source_task_id, recovery.quick_action_id, recovery.via_plugin_id, recovery.revision
FROM comment recovery
JOIN agent_task_queue failed ON failed.id = recovery.source_task_id
JOIN agent_task_queue source ON source.id = failed.delegated_from_task_id
JOIN issue source_issue ON source_issue.id = source.issue_id
JOIN agent source_agent ON source_agent.id = source.agent_id
WHERE recovery.author_type = 'system'
  AND recovery.type = 'progress_update'
  AND recovery.source_task_id IS NOT NULL
  AND recovery.issue_id = source_issue.id
  AND recovery.workspace_id = source_issue.workspace_id
  AND failed.status = 'failed'
  AND failed.delegated_from_task_id IS NOT NULL
  AND failed.autopilot_run_id IS NULL
  AND failed.trigger_evidence_kind IS DISTINCT FROM 'delegated_failure'
  AND source.autopilot_run_id IS NULL
  AND source.issue_id IS NOT NULL
  AND source.agent_id <> failed.agent_id
  AND issue_effective_status(source_issue.workspace_id, source_issue.status) NOT IN ('done', 'cancelled', 'backlog')
  AND source_agent.archived_at IS NULL
  AND source_agent.runtime_id IS NOT NULL
  AND source_agent.workspace_id = source_issue.workspace_id
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue retry
      WHERE retry.parent_task_id = failed.id
        AND retry.status <> 'cancelled'
  )
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue covering
      WHERE covering.issue_id = source_issue.id
        AND covering.agent_id = source.agent_id
        AND (
            recovery.id = ANY(covering.delivered_comment_ids)
            OR (
                (
                    covering.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
                    OR (
                        covering.status = 'deferred'
                        AND covering.context->>'channel_issue_media_pending' = 'true'
                    )
                )
                AND (
                    covering.trigger_comment_id = recovery.id
                    OR recovery.id = ANY(covering.coalesced_comment_ids)
                )
            )
        )
  )
ORDER BY recovery.created_at ASC, recovery.id ASC
LIMIT $1"#
    )
        .bind(max_per_tick)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn list_pending_tasks_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE runtime_id = $1 AND status IN ('queued', 'dispatched')
ORDER BY priority DESC, created_at ASC"#
    )
        .bind(runtime_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_queued_claim_candidates_by_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE runtime_id = $1
  AND status = 'queued'
  AND (
      issue_id IS NULL
      OR dependency_graph_issue_gate_open(
          (SELECT i.workspace_id FROM issue i WHERE i.id = agent_task_queue.issue_id),
          issue_id
      )
  )
ORDER BY priority DESC, created_at ASC"#
    )
        .bind(runtime_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_queued_claim_candidates_by_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE runtime_id = ANY($1::uuid[])
  AND status = 'queued'
  AND (
      issue_id IS NULL
      OR dependency_graph_issue_gate_open(
          (SELECT i.workspace_id FROM issue i WHERE i.id = agent_task_queue.issue_id),
          issue_id
      )
  )
ORDER BY priority DESC, created_at ASC"#
    )
        .bind(runtime_ids)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_tasks_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE issue_id = $1
ORDER BY created_at DESC"#
    )
        .bind(issue_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn list_user_agents_by_runtime_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Agent>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE runtime_id = $1 AND kind = 'user'
ORDER BY id
FOR UPDATE"#
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

pub async fn list_workspace_agent_task_snapshot(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"SELECT atq.id, atq.agent_id, atq.issue_id, atq.status, atq.priority, atq.dispatched_at, atq.started_at, atq.completed_at, atq.result, atq.error, atq.created_at, atq.context, atq.runtime_id, atq.session_id, atq.work_dir, atq.trigger_comment_id, atq.chat_session_id, atq.autopilot_run_id, atq.attempt, atq.max_attempts, atq.parent_task_id, atq.failure_reason, atq.trigger_summary, atq.force_fresh_session, atq.is_leader_task, atq.wait_reason, atq.initiator_user_id, atq.handoff_note, atq.prepare_lease_expires_at, atq.team_id, atq.runtime_mcp_overlay, atq.escalation_for_task_id, atq.fire_at, atq.originator_user_id, atq.runtime_connected_apps, atq.coalesced_comment_ids, atq.delivered_comment_ids, atq.chat_input_task_id, atq.chat_finalize_deferred_at, atq.originator_source, atq.delegated_from_task_id, atq.retry_of_task_id, atq.rerun_of_task_id, atq.rule_version_id, atq.trigger_evidence_kind, atq.trigger_evidence_ref_id, atq.accountable_user_id, atq.session_rollout_missing, atq.retired_session_id, atq.quick_actions_disabled, atq.regenerate_quick_actions_for, atq.branch_name, atq.durable_work_dir, execution_lane_key FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
WHERE a.workspace_id = $1
  AND atq.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')

UNION ALL

SELECT latest.id, latest.agent_id, latest.issue_id, latest.status, latest.priority, latest.dispatched_at, latest.started_at, latest.completed_at, latest.result, latest.error, latest.created_at, latest.context, latest.runtime_id, latest.session_id, latest.work_dir, latest.trigger_comment_id, latest.chat_session_id, latest.autopilot_run_id, latest.attempt, latest.max_attempts, latest.parent_task_id, latest.failure_reason, latest.trigger_summary, latest.force_fresh_session, latest.is_leader_task, latest.wait_reason, latest.initiator_user_id, latest.handoff_note, latest.prepare_lease_expires_at, latest.team_id, latest.runtime_mcp_overlay, latest.escalation_for_task_id, latest.fire_at, latest.originator_user_id, latest.runtime_connected_apps, latest.coalesced_comment_ids, latest.delivered_comment_ids, latest.chat_input_task_id, latest.chat_finalize_deferred_at, latest.originator_source, latest.delegated_from_task_id, latest.retry_of_task_id, latest.rerun_of_task_id, latest.rule_version_id, latest.trigger_evidence_kind, latest.trigger_evidence_ref_id, latest.accountable_user_id, latest.session_rollout_missing, latest.retired_session_id, latest.quick_actions_disabled, latest.regenerate_quick_actions_for, latest.branch_name, latest.durable_work_dir, execution_lane_key FROM agent a
JOIN LATERAL (
  SELECT atq.id, atq.agent_id, atq.issue_id, atq.status, atq.priority, atq.dispatched_at, atq.started_at, atq.completed_at, atq.result, atq.error, atq.created_at, atq.context, atq.runtime_id, atq.session_id, atq.work_dir, atq.trigger_comment_id, atq.chat_session_id, atq.autopilot_run_id, atq.attempt, atq.max_attempts, atq.parent_task_id, atq.failure_reason, atq.trigger_summary, atq.force_fresh_session, atq.is_leader_task, atq.wait_reason, atq.initiator_user_id, atq.handoff_note, atq.prepare_lease_expires_at, atq.team_id, atq.runtime_mcp_overlay, atq.escalation_for_task_id, atq.fire_at, atq.originator_user_id, atq.runtime_connected_apps, atq.coalesced_comment_ids, atq.delivered_comment_ids, atq.chat_input_task_id, atq.chat_finalize_deferred_at, atq.originator_source, atq.delegated_from_task_id, atq.retry_of_task_id, atq.rerun_of_task_id, atq.rule_version_id, atq.trigger_evidence_kind, atq.trigger_evidence_ref_id, atq.accountable_user_id, atq.session_rollout_missing, atq.retired_session_id, atq.quick_actions_disabled, atq.regenerate_quick_actions_for, atq.branch_name, atq.durable_work_dir, atq.execution_lane_key
  FROM agent_task_queue atq
  WHERE atq.agent_id = a.id
    AND atq.status IN ('completed', 'failed')
  ORDER BY atq.completed_at DESC NULLS LAST, atq.created_at DESC, atq.id DESC
  LIMIT 1
) latest ON TRUE
WHERE a.workspace_id = $1"#
    )
        .bind(workspace_id)
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
            autopilot_run_id: row.try_get(17)?,
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListWorkspaceWorkingAgentsRow {
    pub id: Option<Uuid>,
    pub name: String,
    pub avatar_url: Option<String>,
    pub running_task_count: i32,
    pub issue_ids: Option<Vec<Uuid>>,
}

pub async fn list_workspace_working_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    work_type: &str,
    mine_relation: &str,
    member_id: Option<Uuid>,
    parent_issue_id: Option<Uuid>,
) -> anyhow::Result<Vec<ListWorkspaceWorkingAgentsRow>> {
    let rows = sqlx::query(
        r#"SELECT
  a.id,
  a.name,
  a.avatar_url,
  COUNT(*)::int AS running_task_count,
  COALESCE(
    ARRAY_AGG(DISTINCT atq.issue_id ORDER BY atq.issue_id)
      FILTER (WHERE atq.issue_id IS NOT NULL),
    ARRAY[]::uuid[]
  )::uuid[] AS issue_ids
FROM agent a
JOIN agent_task_queue atq ON atq.agent_id = a.id
WHERE a.workspace_id = $1
  AND a.kind = 'user'
  AND a.archived_at IS NULL
  AND atq.status = 'running'
  AND (
    $2::text = ''
    OR ($2::text = 'chat' AND atq.chat_session_id IS NOT NULL)
    OR (
      $2::text = 'autopilot'
      AND atq.chat_session_id IS NULL
      AND atq.autopilot_run_id IS NOT NULL
    )
    OR (
      $2::text = 'issue'
      AND atq.chat_session_id IS NULL
      AND atq.autopilot_run_id IS NULL
      AND atq.issue_id IS NOT NULL
    )
  )
  AND (
    $3::text = ''
    OR EXISTS (
      SELECT 1
      FROM issue i
      WHERE i.id = atq.issue_id
        AND i.workspace_id = a.workspace_id
        AND (
          (
            $3::text IN ('assigned', 'any')
            AND i.assignee_type = 'member'
            AND i.assignee_id = $4::uuid
          )
          OR (
            $3::text IN ('created', 'any')
            AND i.creator_type = 'member'
            AND i.creator_id = $4::uuid
          )
          OR (
            $3::text IN ('involved', 'any')
            AND (
              (
                i.assignee_type = 'agent'
                AND EXISTS (
                  SELECT 1
                  FROM agent owned_agent
                  WHERE owned_agent.id = i.assignee_id
                    AND owned_agent.workspace_id = a.workspace_id
                    AND owned_agent.owner_id = $4::uuid
                )
              )
              OR (
                i.assignee_type = 'team'
                AND EXISTS (
                  SELECT 1
                  FROM team s
                  WHERE s.id = i.assignee_id
                    AND s.workspace_id = a.workspace_id
                    AND (
                      EXISTS (
                        SELECT 1
                        FROM team_member sm
                        WHERE sm.team_id = s.id
                          AND sm.member_type = 'member'
                          AND sm.member_id = $4::uuid
                      )
                      OR EXISTS (
                        SELECT 1
                        FROM agent leader
                        WHERE leader.id = s.leader_id
                          AND leader.workspace_id = a.workspace_id
                          AND leader.owner_id = $4::uuid
                      )
                      OR EXISTS (
                        SELECT 1
                        FROM team_member sm
                        JOIN agent owned_member ON owned_member.id = sm.member_id
                        WHERE sm.team_id = s.id
                          AND sm.member_type = 'agent'
                          AND owned_member.workspace_id = a.workspace_id
                          AND owned_member.owner_id = $4::uuid
                      )
                    )
                )
              )
            )
          )
        )
    )
  )
  AND (
    $5::uuid IS NULL
    OR EXISTS (
      SELECT 1
      FROM issue child
      WHERE child.id = atq.issue_id
        AND child.workspace_id = a.workspace_id
        AND child.parent_issue_id = $5::uuid
    )
  )
GROUP BY a.id, a.name, a.avatar_url, a.created_at
ORDER BY a.created_at ASC"#,
    )
    .bind(workspace_id)
    .bind(work_type)
    .bind(mine_relation)
    .bind(member_id)
    .bind(parent_issue_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListWorkspaceWorkingAgentsRow {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
            avatar_url: row.try_get(2)?,
            running_task_count: row.try_get(3)?,
            issue_ids: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn lock_agent_for_autopilot_assignment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier FROM agent
WHERE id = $1 AND workspace_id = $2 AND kind = 'user'
FOR SHARE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn mark_agent_task_waiting_local_directory(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    wait_reason: Option<&str>,
    prepare_lease_secs: f64,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'waiting_local_directory',
    wait_reason = $2,
    prepare_lease_expires_at = now() + make_interval(secs => $3::double precision)
WHERE id = $1 AND status = 'dispatched'
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .bind(wait_reason)
        .bind(prepare_lease_secs)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn mark_chat_finalize_deferred(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET chat_finalize_deferred_at = now()
WHERE id = $1
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MergeCommentIntoPendingTaskRow {
    pub id: Option<Uuid>,
    pub coalesced_comment_ids: Option<Vec<Uuid>>,
}

pub async fn merge_comment_into_pending_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    new_trigger_comment_id: Uuid,
    new_trigger_summary: Option<&str>,
    new_originator_user_id: Uuid,
    new_accountable_user_id: Uuid,
    new_originator_source: Option<&str>,
    new_delegated_from_task_id: Uuid,
    new_rule_version_id: Uuid,
    new_trigger_evidence_kind: Option<&str>,
    new_trigger_evidence_ref_id: Uuid,
    new_runtime_mcp_overlay: &serde_json::Value,
    new_runtime_connected_apps: &serde_json::Value,
    issue_id: Uuid,
    agent_id: Uuid,
    head_sha: Option<&str>,
) -> anyhow::Result<Option<MergeCommentIntoPendingTaskRow>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET coalesced_comment_ids = (
        SELECT COALESCE(array_agg(DISTINCT e), '{}')
        FROM unnest(array_append(coalesced_comment_ids, trigger_comment_id)) AS e
        WHERE e IS NOT NULL AND e <> $1::uuid
    ),
    trigger_comment_id = $1::uuid,
    trigger_summary = COALESCE($2, trigger_summary),
    -- Re-attribution is ATOMIC (PB-4302): folding a newly-arrived comment moves the
    -- WHOLE attribution snapshot to that comment's human — person columns, source
    -- label, delegation lineage, rule version, and evidence — computed by the caller
    -- as one attribution.Result. Re-stamping only the person columns would leave a
    -- run showing B accountable while still pointing at A's stale source / evidence /
    -- level. accountable comes from the resolved Result (finalizeAttribution already
    -- guaranteed originator ⟹ accountable == originator; the cross-column CHECK backs it).
    originator_user_id = $3::uuid,
    accountable_user_id = $4::uuid,
    originator_source = $5,
    delegated_from_task_id = $6::uuid,
    rule_version_id = $7::uuid,
    trigger_evidence_kind = $8,
    trigger_evidence_ref_id = $9::uuid,
    runtime_mcp_overlay = $10,
    runtime_connected_apps = $11
WHERE id = (
    SELECT t.id FROM agent_task_queue t
    WHERE t.issue_id = $12
      AND t.agent_id = $13
      AND (
          t.status = 'queued'
          OR (t.status = 'deferred' AND t.context->>'channel_issue_media_pending' = 'true')
      )
      -- Head-scoped (TEN-356, #5914): never fold across HEADs. The physical
      -- unique index is only (issue_id, agent_id), so an insert-race loser can
      -- collide with a pending task stamped for a DIFFERENT head_sha; merging
      -- into it would give a new-HEAD comment old-HEAD review coverage. Empty/
      -- absent head_sha (no linked PR) matches any task, preserving coalescing.
      AND (
          COALESCE($14::text, '') = ''
          OR t.context->>'head_sha' = $14::text
      )
    ORDER BY t.created_at DESC
    LIMIT 1
)
RETURNING id, coalesced_comment_ids"#,
    )
    .bind(new_trigger_comment_id)
    .bind(new_trigger_summary)
    .bind(new_originator_user_id)
    .bind(new_accountable_user_id)
    .bind(new_originator_source)
    .bind(new_delegated_from_task_id)
    .bind(new_rule_version_id)
    .bind(new_trigger_evidence_kind)
    .bind(new_trigger_evidence_ref_id)
    .bind(new_runtime_mcp_overlay)
    .bind(new_runtime_connected_apps)
    .bind(issue_id)
    .bind(agent_id)
    .bind(head_sha)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(MergeCommentIntoPendingTaskRow {
        id: row.try_get(0)?,
        coalesced_comment_ids: row.try_get(1)?,
    }))
}

pub async fn merge_delegated_failure_comment_into_pending_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    trigger_summary: Option<&str>,
    issue_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET coalesced_comment_ids = (
        SELECT COALESCE(array_agg(DISTINCT e), '{}')
        FROM unnest(array_append(coalesced_comment_ids, trigger_comment_id)) AS e
        WHERE e IS NOT NULL AND e <> $1::uuid
    ),
    trigger_comment_id = $1::uuid,
    trigger_summary = $2
WHERE id = (
    SELECT t.id FROM agent_task_queue t
    WHERE t.issue_id = $3
      AND t.agent_id = $4
      AND (
          t.status = 'queued'
          OR (t.status = 'deferred' AND t.context->>'channel_issue_media_pending' = 'true')
      )
      AND t.trigger_comment_id IS DISTINCT FROM $1::uuid
      AND NOT ($1::uuid = ANY(t.coalesced_comment_ids))
    ORDER BY t.created_at DESC
    LIMIT 1
)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(comment_id)
        .bind(trigger_summary)
        .bind(issue_id)
        .bind(agent_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn promote_deferred_channel_issue_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue AS task
SET status = 'queued', fire_at = NULL
WHERE task.id = $1
  AND task.issue_id IS NOT NULL
  AND task.status = 'deferred'
  AND dependency_graph_issue_gate_open(
      (SELECT i.workspace_id FROM issue i WHERE i.id = task.issue_id),
      task.issue_id
  )
  AND task.context->>'coordination_assignment_id' IS NULL
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue occupant
      WHERE occupant.execution_lane_key = task.execution_lane_key
        AND occupant.id <> task.id
        AND (
            occupant.status IN ('queued', 'dispatched')
            OR (occupant.status = 'deferred'
                AND occupant.context->>'channel_issue_media_pending' = 'true')
        )
  )
RETURNING task.id, task.agent_id, task.issue_id, task.status, task.priority, task.dispatched_at, task.started_at, task.completed_at, task.result, task.error, task.created_at, task.context, task.runtime_id, task.session_id, task.work_dir, task.trigger_comment_id, task.chat_session_id, task.autopilot_run_id, task.attempt, task.max_attempts, task.parent_task_id, task.failure_reason, task.trigger_summary, task.force_fresh_session, task.is_leader_task, task.wait_reason, task.initiator_user_id, task.handoff_note, task.prepare_lease_expires_at, task.team_id, task.runtime_mcp_overlay, task.escalation_for_task_id, task.fire_at, task.originator_user_id, task.runtime_connected_apps, task.coalesced_comment_ids, task.delivered_comment_ids, task.chat_input_task_id, task.chat_finalize_deferred_at, task.originator_source, task.delegated_from_task_id, task.retry_of_task_id, task.rerun_of_task_id, task.rule_version_id, task.trigger_evidence_kind, task.trigger_evidence_ref_id, task.accountable_user_id, task.session_rollout_missing, task.retired_session_id, task.quick_actions_disabled, task.regenerate_quick_actions_for, task.branch_name, task.durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn promote_due_deferred_tasks_for_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    runtime_stale_secs: f64,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH due AS (
    SELECT t.id,
           row_number() OVER (
               PARTITION BY t.execution_lane_key
               ORDER BY t.priority DESC, t.created_at ASC, t.id
           ) AS rn
    FROM agent_task_queue t
    WHERE t.runtime_id = $1
      AND t.status = 'deferred'
      AND t.fire_at <= now()
      AND (
        t.issue_id IS NULL
        OR dependency_graph_issue_gate_open(
            (SELECT i.workspace_id FROM issue i WHERE i.id = t.issue_id),
            t.issue_id
        )
      )
      AND (
        COALESCE(t.context->>'message_bus_parent_task_id', '') = ''
        OR EXISTS (
            SELECT 1 FROM agent_task_queue parent
            WHERE parent.id::text = t.context->>'message_bus_parent_task_id'
              AND parent.status IN ('completed', 'failed', 'cancelled')
        )
      )
      AND (
        COALESCE(t.context->>'message_bus_parent_task_id', '') = ''
        OR NOT EXISTS (
          SELECT 1 FROM agent_task_queue active
          WHERE active.execution_lane_key = t.execution_lane_key
            AND active.id <> t.id
            AND active.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
        )
      )
      AND EXISTS (
        SELECT 1 FROM agent_runtime r
        WHERE r.id = t.runtime_id
          AND r.status = 'online'
          AND COALESCE(r.last_seen_at, r.updated_at) >=
              now() - make_interval(secs => $2::double precision)
      )
      AND NOT EXISTS (
        SELECT 1 FROM agent_task_queue occupant
        WHERE occupant.execution_lane_key = t.execution_lane_key
          AND occupant.id <> t.id
          AND (
            occupant.status IN ('queued', 'dispatched')
            OR (occupant.status = 'deferred' AND occupant.context->>'channel_issue_media_pending' = 'true')
          )
      )
)
UPDATE agent_task_queue
SET status = 'queued'
WHERE id IN (SELECT id FROM due WHERE rn = 1)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(runtime_id)
        .bind(runtime_stale_secs)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn promote_due_deferred_tasks_for_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_ids: Vec<Uuid>,
    runtime_stale_secs: f64,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"WITH due AS (
    SELECT t.id,
           row_number() OVER (
               PARTITION BY t.execution_lane_key
               ORDER BY t.priority DESC, t.created_at ASC, t.id
           ) AS rn
    FROM agent_task_queue t
    WHERE t.runtime_id = ANY($1::uuid[])
      AND t.status = 'deferred'
      AND t.fire_at <= now()
      AND (
        t.issue_id IS NULL
        OR dependency_graph_issue_gate_open(
            (SELECT i.workspace_id FROM issue i WHERE i.id = t.issue_id),
            t.issue_id
        )
      )
      AND (
        COALESCE(t.context->>'message_bus_parent_task_id', '') = ''
        OR EXISTS (
            SELECT 1 FROM agent_task_queue parent
            WHERE parent.id::text = t.context->>'message_bus_parent_task_id'
              AND parent.status IN ('completed', 'failed', 'cancelled')
        )
      )
      AND (
        COALESCE(t.context->>'message_bus_parent_task_id', '') = ''
        OR NOT EXISTS (
          SELECT 1 FROM agent_task_queue active
          WHERE active.execution_lane_key = t.execution_lane_key
            AND active.id <> t.id
            AND active.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
        )
      )
      AND EXISTS (
        SELECT 1 FROM agent_runtime r
        WHERE r.id = t.runtime_id
          AND r.status = 'online'
          AND COALESCE(r.last_seen_at, r.updated_at) >=
              now() - make_interval(secs => $2::double precision)
      )
      AND NOT EXISTS (
        SELECT 1 FROM agent_task_queue occupant
        WHERE occupant.execution_lane_key = t.execution_lane_key
          AND occupant.id <> t.id
          AND (
            occupant.status IN ('queued', 'dispatched')
            OR (occupant.status = 'deferred' AND occupant.context->>'channel_issue_media_pending' = 'true')
          )
      )
)
UPDATE agent_task_queue
SET status = 'queued'
WHERE id IN (SELECT id FROM due WHERE rn = 1)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(runtime_ids)
        .bind(runtime_stale_secs)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn rebind_agent_builder_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    runtime_mode: &str,
    model: Option<&str>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent
SET runtime_id = $1,
    runtime_mode = $2,
    model = $3,
    updated_at = now()
WHERE id = $4 AND kind = 'system' AND system_key LIKE 'agent_builder:%'
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(runtime_id)
        .bind(runtime_mode)
        .bind(model)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn reclaim_stale_dispatched_task_for_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    prepare_lease_secs: f64,
    claim_recovery_secs: f64,
    runtime_stale_secs: f64,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET dispatched_at = now(),
    prepare_lease_expires_at = now() + make_interval(secs => $2::double precision)
WHERE id = (
    SELECT atq.id FROM agent_task_queue atq
    WHERE atq.runtime_id = $1
      AND atq.status = 'dispatched'
      AND atq.started_at IS NULL
      AND atq.dispatched_at < now() - make_interval(secs => $3::double precision)
      AND (atq.prepare_lease_expires_at IS NULL OR atq.prepare_lease_expires_at < now())
      AND EXISTS (
          SELECT 1 FROM agent_runtime r
          WHERE r.id = atq.runtime_id
            AND r.status = 'online'
            AND COALESCE(r.last_seen_at, r.updated_at) >=
                now() - make_interval(secs => $4::double precision)
      )
    ORDER BY atq.priority DESC, atq.dispatched_at ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(runtime_id)
        .bind(prepare_lease_secs)
        .bind(claim_recovery_secs)
        .bind(runtime_stale_secs)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn reclaim_stale_dispatched_tasks_for_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    prepare_lease_secs: f64,
    runtime_ids: Vec<Uuid>,
    claim_recovery_secs: f64,
    runtime_stale_secs: f64,
    max_tasks: i32,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET dispatched_at = now(),
    prepare_lease_expires_at = now() + make_interval(secs => $1::double precision)
WHERE id IN (
    SELECT atq.id FROM agent_task_queue atq
    WHERE atq.runtime_id = ANY($2::uuid[])
      AND atq.status = 'dispatched'
      AND atq.started_at IS NULL
      AND atq.dispatched_at < now() - make_interval(secs => $3::double precision)
      AND (atq.prepare_lease_expires_at IS NULL OR atq.prepare_lease_expires_at < now())
      AND EXISTS (
          SELECT 1 FROM agent_runtime r
          WHERE r.id = atq.runtime_id
            AND r.status = 'online'
            AND COALESCE(r.last_seen_at, r.updated_at) >=
                now() - make_interval(secs => $4::double precision)
      )
    ORDER BY atq.priority DESC, atq.dispatched_at ASC
    LIMIT $5::int
    FOR UPDATE SKIP LOCKED
)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(prepare_lease_secs)
        .bind(runtime_ids)
        .bind(claim_recovery_secs)
        .bind(runtime_stale_secs)
        .bind(max_tasks)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn recover_orphaned_tasks_for_runtime(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'failed',
    completed_at = now(),
    error = 'daemon restarted while task was in flight',
    failure_reason = 'runtime_recovery',
    wait_reason = NULL,
    prepare_lease_expires_at = NULL
WHERE runtime_id = $1 AND status IN ('dispatched', 'running', 'waiting_local_directory')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(runtime_id)
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
            autopilot_run_id: row.try_get(17)?,
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

pub async fn refresh_agent_status_from_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"WITH desired AS (
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM agent_task_queue q
        WHERE q.agent_id = $1 AND q.status IN ('dispatched', 'running')
    ) THEN 'working' ELSE 'idle' END AS status
)
UPDATE agent AS a
SET status = desired.status,
    updated_at = now()
FROM desired
WHERE a.id = $1 AND a.status IS DISTINCT FROM desired.status
RETURNING a.id, a.workspace_id, a.name, a.avatar_url, a.runtime_mode, a.runtime_config, a.visibility, a.status, a.max_concurrent_tasks, a.owner_id, a.created_at, a.updated_at, a.description, a.runtime_id, a.instructions, a.archived_at, a.archived_by, a.custom_env, a.custom_args, a.mcp_config, a.model, a.thinking_level, a.composio_toolkit_allowlist, a.permission_mode, a.kind, a.system_key, a.disabled_runtime_skills, a.service_tier"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RegisterPlannedCommentForActiveTaskRow {
    pub id: Option<Uuid>,
    pub coalesced_comment_ids: Option<Vec<Uuid>>,
}

pub async fn register_planned_comment_for_active_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    issue_id: Uuid,
    agent_id: Uuid,
    head_sha: Option<&str>,
) -> anyhow::Result<Option<RegisterPlannedCommentForActiveTaskRow>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET coalesced_comment_ids = (
        SELECT COALESCE(array_agg(DISTINCT e), '{}')
        FROM unnest(array_append(coalesced_comment_ids, $1::uuid)) AS e
        WHERE e IS NOT NULL
    )
WHERE id = (
    SELECT t.id FROM agent_task_queue t
    WHERE t.execution_lane_key = 'issue:' || $2::text || ':agent:' || $3::text || ':main'
      AND t.status IN ('dispatched', 'running', 'waiting_local_directory')
      AND (
          COALESCE($4::text, '') = ''
          OR t.context->>'head_sha' = $4::text
      )
    ORDER BY t.created_at DESC
    LIMIT 1
)
RETURNING id, coalesced_comment_ids"#,
    )
    .bind(comment_id)
    .bind(issue_id)
    .bind(agent_id)
    .bind(head_sha)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RegisterPlannedCommentForActiveTaskRow {
        id: row.try_get(0)?,
        coalesced_comment_ids: row.try_get(1)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveIssueAgentTaskRow {
    pub id: Uuid,
    pub status: String,
}

pub async fn get_active_issue_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<ActiveIssueAgentTaskRow>> {
    let row = sqlx::query(
        r#"SELECT id, status
FROM agent_task_queue
WHERE execution_lane_key = 'issue:' || $1::text || ':agent:' || $2::text || ':main'
  AND status IN ('dispatched', 'running', 'waiting_local_directory')
ORDER BY started_at DESC NULLS LAST, created_at DESC
LIMIT 1"#,
    )
    .bind(issue_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ActiveIssueAgentTaskRow {
        id: row.try_get(0)?,
        status: row.try_get(1)?,
    }))
}

/// Serializes Message Bus writes for one main task. The row lock closes the
/// race where two Side Chats both observe that no deferred continuation exists
/// and create duplicate children.
pub async fn lock_task_for_message_bus(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"SELECT id
FROM agent_task_queue
WHERE id = $1
  AND lock_task_owner_rows(agent_id, issue_id, runtime_id)
FOR UPDATE"#,
    )
    .bind(task_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

/// Appends an instruction to the still-deferred continuation for a main task.
/// Messages stay structured so the eventual prompt can preserve provenance.
pub async fn append_task_message_bus_instruction(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    parent_task_id: Uuid,
    source_task_id: Uuid,
    source_trigger_comment_id: Uuid,
    message_id: Uuid,
    content: &str,
) -> anyhow::Result<Option<Uuid>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET context = jsonb_set(
        COALESCE(context, '{}'::jsonb),
        '{message_bus_messages}',
        COALESCE(context->'message_bus_messages', '[]'::jsonb)
            || jsonb_build_array(jsonb_build_object(
                'id', $4::text,
                'source_task_id', $2::text,
                'content', $5::text
            )),
        TRUE
    ),
    coalesced_comment_ids = CASE
        WHEN $3::uuid = '00000000-0000-0000-0000-000000000000'::uuid
          OR trigger_comment_id = $3::uuid
          OR $3::uuid = ANY(coalesced_comment_ids)
        THEN coalesced_comment_ids
        ELSE array_append(coalesced_comment_ids, $3::uuid)
    END
WHERE id = (
    SELECT id
    FROM agent_task_queue
    WHERE status = 'deferred'
      AND context->>'message_bus_parent_task_id' = $1::text
    ORDER BY created_at DESC
    LIMIT 1
    FOR UPDATE
)
RETURNING id"#,
    )
    .bind(parent_task_id)
    .bind(source_task_id)
    .bind(source_trigger_comment_id)
    .bind(message_id)
    .bind(content)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

/// Creates a provider-neutral continuation of the exact main task. It remains
/// deferred until the normal promoter observes the named parent in a terminal
/// state. Unlike a retry, this is a deliberate new turn: attempt/retry lineage
/// is not incremented or forged.
pub async fn create_task_message_bus_continuation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    parent_task_id: Uuid,
    source_task_id: Uuid,
    source_trigger_comment_id: Uuid,
    message_id: Uuid,
    content: &str,
    task_id: Uuid,
) -> anyhow::Result<Option<Uuid>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    id, agent_id, runtime_id, issue_id, status, priority,
    trigger_comment_id, trigger_summary, context, session_id, work_dir,
    attempt, max_attempts, parent_task_id, force_fresh_session,
    is_leader_task, team_id, originator_user_id, accountable_user_id,
    runtime_mcp_overlay, runtime_connected_apps, originator_source,
    delegated_from_task_id, rule_version_id, trigger_evidence_kind,
    trigger_evidence_ref_id, fire_at
)
SELECT
    $6, parent.agent_id, parent.runtime_id, parent.issue_id, 'deferred', parent.priority,
    NULLIF($3::uuid, '00000000-0000-0000-0000-000000000000'::uuid), LEFT($5::text, 200),
    (COALESCE(parent.context, '{}'::jsonb)
        - 'side_chat_parent_task_id'
        - 'side_chat_root_comment_id'
        - 'channel_issue_media_pending'
        - 'message_bus_parent_task_id'
        - 'message_bus_messages') || jsonb_build_object(
            'message_bus_parent_task_id', parent.id::text,
            'message_bus_messages', jsonb_build_array(jsonb_build_object(
                'id', $4::text,
                'source_task_id', $2::text,
                'content', $5::text
            ))
        ),
    parent.session_id, parent.work_dir,
    1, parent.max_attempts, NULL, FALSE,
    parent.is_leader_task, parent.team_id, parent.originator_user_id,
    parent.accountable_user_id, parent.runtime_mcp_overlay,
    parent.runtime_connected_apps, parent.originator_source,
    parent.delegated_from_task_id, parent.rule_version_id,
    parent.trigger_evidence_kind, parent.trigger_evidence_ref_id, now()
FROM agent_task_queue parent
WHERE parent.id = $1
  AND parent.issue_id IS NOT NULL
  AND parent.execution_lane_key =
      'issue:' || parent.issue_id::text || ':agent:' || parent.agent_id::text || ':main'
  AND lock_task_owner_rows(parent.agent_id, parent.issue_id, parent.runtime_id)
RETURNING id"#,
    )
    .bind(parent_task_id)
    .bind(source_task_id)
    .bind(source_trigger_comment_id)
    .bind(message_id)
    .bind(content)
    .bind(task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn requeue_agent_task_after_claim_failure(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    runtime_id: Uuid,
    dispatched_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'queued',
    dispatched_at = NULL,
    prepare_lease_expires_at = NULL,
    delivered_comment_ids = '{}'
WHERE id = $1
  AND runtime_id = $2
  AND status = 'dispatched'
  AND started_at IS NULL
  AND dispatched_at = $3
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(task_id)
        .bind(runtime_id)
        .bind(dispatched_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn restore_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET archived_at = NULL, archived_by = NULL, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn set_agent_task_branch_name(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    branch_name: Option<&str>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET branch_name = COALESCE(branch_name, $1)
WHERE id = $2 AND status = 'cancelled'"#,
    )
    .bind(branch_name)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_agent_task_durable_work_dir(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    durable_work_dir: Option<&str>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET durable_work_dir = COALESCE(durable_work_dir, $1)
WHERE id = $2 AND status = 'cancelled'"#,
    )
    .bind(durable_work_dir)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_agent_task_error_if_empty(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    error: Option<&str>,
    failure_reason: Option<&str>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET error = $1,
    failure_reason = COALESCE(failure_reason, $2)
WHERE id = $3 AND (error IS NULL OR error = '') AND status = 'cancelled'"#,
    )
    .bind(error)
    .bind(failure_reason)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn mark_cancelled_task_session_rollout_missing(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE agent_task_queue
SET session_id = NULL,
    session_rollout_missing = TRUE
WHERE id = $1 AND status = 'cancelled'"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn set_deferred_channel_issue_task_runtime_overlay(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_mcp_overlay: &serde_json::Value,
    runtime_connected_apps: &serde_json::Value,
    id: Uuid,
    expected_originator_user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET runtime_mcp_overlay = $1,
    runtime_connected_apps = $2
WHERE id = $3
  AND status = 'deferred'
  AND context->>'channel_issue_media_pending' = 'true'
  AND trigger_comment_id IS NULL
  AND originator_user_id IS NOT DISTINCT FROM $4::uuid"#,
    )
    .bind(runtime_mcp_overlay)
    .bind(runtime_connected_apps)
    .bind(id)
    .bind(expected_originator_user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_task_delivered_comment_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    delivered_comment_ids: Vec<Uuid>,
    task_id: Uuid,
    runtime_id: Uuid,
    dispatched_at: Option<DateTime<Utc>>,
    expected_trigger_comment_id: Uuid,
) -> anyhow::Result<Vec<Option<Vec<Uuid>>>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue
SET delivered_comment_ids = $1::uuid[]
WHERE id = $2
  AND runtime_id = $3
  AND status = 'dispatched'
  AND started_at IS NULL
  AND dispatched_at = $4
  AND trigger_comment_id IS NOT DISTINCT FROM $5::uuid
  AND NOT EXISTS (
      SELECT 1
      FROM unnest($1::uuid[]) AS delivered(id)
      WHERE delivered.id IS NULL
         OR (
             delivered.id IS DISTINCT FROM trigger_comment_id
             AND NOT (delivered.id = ANY(coalesced_comment_ids))
         )
  )
RETURNING delivered_comment_ids"#,
    )
    .bind(delivered_comment_ids)
    .bind(task_id)
    .bind(runtime_id)
    .bind(dispatched_at)
    .bind(expected_trigger_comment_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn start_agent_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'running',
    started_at = now(),
    wait_reason = NULL,
    prepare_lease_expires_at = NULL
WHERE id = $1 AND status IN ('dispatched', 'waiting_local_directory')
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentTaskQueue {
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
        autopilot_run_id: row.try_get(17)?,
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
    }))
}

pub async fn update_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    avatar_url: Option<&str>,
    runtime_config: &serde_json::Value,
    runtime_mode: Option<&str>,
    runtime_id: Option<Uuid>,
    visibility: Option<&str>,
    permission_mode: Option<&str>,
    status: Option<&str>,
    max_concurrent_tasks: Option<i32>,
    instructions: Option<&str>,
    custom_env: &serde_json::Value,
    custom_args: &serde_json::Value,
    mcp_config: &serde_json::Value,
    model: Option<&str>,
    thinking_level: Option<&str>,
    service_tier: Option<&str>,
    composio_toolkit_allowlist: Option<&[String]>,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET
    name = COALESCE($2, name),
    description = COALESCE($3, description),
    avatar_url = COALESCE($4, avatar_url),
    runtime_config = COALESCE($5, runtime_config),
    runtime_mode = COALESCE($6, runtime_mode),
    runtime_id = COALESCE($7, runtime_id),
    visibility = COALESCE($8, visibility),
    permission_mode = COALESCE($9, permission_mode),
    status = COALESCE($10, status),
    max_concurrent_tasks = COALESCE($11, max_concurrent_tasks),
    instructions = COALESCE($12, instructions),
    custom_env = COALESCE($13, custom_env),
    custom_args = COALESCE($14, custom_args),
    mcp_config = COALESCE($15, mcp_config),
    model = COALESCE($16, model),
    thinking_level = COALESCE($17, thinking_level),
    service_tier = COALESCE($18, service_tier),
    composio_toolkit_allowlist = COALESCE($19::text[], composio_toolkit_allowlist),
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(avatar_url)
        .bind(runtime_config)
        .bind(runtime_mode)
        .bind(runtime_id)
        .bind(visibility)
        .bind(permission_mode)
        .bind(status)
        .bind(max_concurrent_tasks)
        .bind(instructions)
        .bind(custom_env)
        .bind(custom_args)
        .bind(mcp_config)
        .bind(model)
        .bind(thinking_level)
        .bind(service_tier)
        .bind(composio_toolkit_allowlist)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn update_agent_custom_env(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    custom_env: &serde_json::Value,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent
SET custom_env = $2, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .bind(custom_env)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn update_agent_disabled_runtime_skills(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    disabled_runtime_skills: &serde_json::Value,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent
SET disabled_runtime_skills = $2, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .bind(disabled_runtime_skills)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn update_agent_status(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    status: &str,
) -> anyhow::Result<Option<Agent>> {
    let row = sqlx::query(
        r#"UPDATE agent SET status = $2, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, avatar_url, runtime_mode, runtime_config, visibility, status, max_concurrent_tasks, owner_id, created_at, updated_at, description, runtime_id, instructions, archived_at, archived_by, custom_env, custom_args, mcp_config, model, thinking_level, composio_toolkit_allowlist, permission_mode, kind, system_key, disabled_runtime_skills, service_tier"#
    )
        .bind(id)
        .bind(status)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Agent {
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
    }))
}

pub async fn update_agent_task_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    session_id: Option<&str>,
    work_dir: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_task_queue
SET session_id = COALESCE($2, session_id),
    work_dir  = COALESCE($3, work_dir)
WHERE id = $1
  AND (
    status IN ('dispatched', 'running')
    OR (status = 'cancelled' AND session_id IS NULL)
  )"#,
    )
    .bind(id)
    .bind(session_id)
    .bind(work_dir)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
