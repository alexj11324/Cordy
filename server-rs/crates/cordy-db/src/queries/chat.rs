//! Typed SQL queries for chat records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn adopt_orphan_onboarding_kickoff(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message
SET task_id = $2
WHERE chat_session_id = $1
  AND role = 'user'
  AND message_kind = 'onboarding_kickoff'
  AND task_id IS NULL"#,
    )
    .bind(chat_session_id)
    .bind(task_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn advance_cancelled_chat_session_pointer(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_session cs
SET session_id = t.session_id,
    runtime_id = t.runtime_id,
    work_dir   = COALESCE(t.work_dir, cs.work_dir),
    updated_at = now()
FROM agent_task_queue t
WHERE t.id = $1
  AND t.chat_session_id = cs.id
  AND t.status = 'cancelled'
  AND t.session_id IS NOT NULL
  AND t.runtime_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM agent_task_queue newer
      WHERE newer.chat_session_id = t.chat_session_id
        AND newer.id <> t.id
        AND newer.session_id IS NOT NULL
        AND newer.created_at > t.created_at
  )"#,
    )
    .bind(task_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn chat_session_has_user_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
    SELECT 1 FROM chat_message
    WHERE chat_session_id = $1 AND role = 'user'
) AS has_user_message"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn clear_chat_message_channel_media_pending(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message
SET channel_media_pending_until = NULL
WHERE id = $1 AND chat_session_id = $2"#,
    )
    .bind(id)
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn clear_chat_session_project_by_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_session
SET project_id = NULL
WHERE project_id = $1 AND workspace_id = $2"#,
    )
    .bind(project_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn clear_chat_session_session_if_matches(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    session_id: Option<&str>,
    runtime_id: Option<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_session
SET session_id = NULL,
    runtime_id = NULL,
    updated_at = now()
WHERE id = $1
  AND session_id = $2
  AND runtime_id = $3"#,
    )
    .bind(id)
    .bind(session_id)
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn create_chat_draft_restore(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    chat_session_id: Uuid,
    task_id: Uuid,
    content: &str,
    attachment_ids: Vec<Uuid>,
) -> anyhow::Result<Option<ChatDraftRestore>> {
    let row = sqlx::query(
        r#"INSERT INTO chat_draft_restore (id, chat_session_id, task_id, content, attachment_ids)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, chat_session_id, task_id, content, attachment_ids, created_at"#,
    )
    .bind(id)
    .bind(chat_session_id)
    .bind(task_id)
    .bind(content)
    .bind(attachment_ids)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatDraftRestore {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        task_id: row.try_get(2)?,
        content: row.try_get(3)?,
        attachment_ids: row.try_get(4)?,
        created_at: row.try_get(5)?,
    }))
}

pub async fn create_chat_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    role: &str,
    content: &str,
    task_id: Option<Uuid>,
    failure_reason: Option<&str>,
    elapsed_ms: Option<i64>,
    message_kind: Option<&str>,
    quick_actions: &serde_json::Value,
    channel_media_pending_secs: Option<f64>,
    channel_ingested: Option<bool>,
    id: Uuid,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"INSERT INTO chat_message (
    chat_session_id, role, content, task_id, failure_reason, elapsed_ms,
    message_kind, quick_actions, channel_media_pending_until, channel_ingested, id
)
VALUES (
    $1, $2, $3, $4, $5, $6,
    COALESCE($7::text, 'message'),
    COALESCE($8::jsonb, '[]'::jsonb),
    -- The media deadline is DB-clock time: every consumer compares it against
    -- SQL now() (GetChannelMediaPendingUntil, the deferred promote, the
    -- trailing-message guard), so the writer must use the same clock. The
    -- caller passes a relative budget in seconds; an application-clock
    -- timestamp here would let a skewed app node shrink or stretch the
    -- fallback window.
    CASE WHEN $9::float8 IS NULL THEN NULL
         ELSE now() + make_interval(secs => $9::float8) END,
    COALESCE($10::boolean, FALSE),
    COALESCE($11::uuid, gen_random_uuid())
)
RETURNING id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions"#
    )
        .bind(chat_session_id)
        .bind(role)
        .bind(content)
        .bind(task_id)
        .bind(failure_reason)
        .bind(elapsed_ms)
        .bind(message_kind)
        .bind(quick_actions)
        .bind(channel_media_pending_secs)
        .bind(channel_ingested)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn create_chat_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
    creator_id: Uuid,
    title: &str,
    is_agent_intro: bool,
    project_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"INSERT INTO chat_session (workspace_id, agent_id, creator_id, title, runtime_id, is_agent_intro, project_id, id)
VALUES ($1, $2, $3, $4, (SELECT runtime_id FROM agent WHERE id = $2), $5,
       NULLIF($6, '00000000-0000-0000-0000-000000000000'::uuid),
       COALESCE(NULLIF($7, '00000000-0000-0000-0000-000000000000'::uuid), gen_random_uuid()))
RETURNING id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id"#
    )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(creator_id)
        .bind(title)
        .bind(is_agent_intro)
        .bind(project_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn create_chat_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    runtime_id: Uuid,
    priority: i32,
    chat_session_id: Uuid,
    initiator_user_id: Uuid,
    fire_at: Option<DateTime<Utc>>,
    originator_user_id: Uuid,
    accountable_user_id: Uuid,
    force_fresh_session: Option<bool>,
    runtime_mcp_overlay: &serde_json::Value,
    runtime_connected_apps: &serde_json::Value,
    originator_source: Option<&str>,
    trigger_evidence_kind: Option<&str>,
    trigger_evidence_ref_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, status, priority, chat_session_id,
    initiator_user_id, originator_user_id, accountable_user_id, force_fresh_session, runtime_mcp_overlay,
    runtime_connected_apps, originator_source, trigger_evidence_kind, trigger_evidence_ref_id,
    fire_at, id
)
SELECT
    $1, $2, NULL,
    CASE WHEN $6::timestamptz IS NULL THEN 'queued' ELSE 'deferred' END,
    $3, $4, $5,
    $7,
    $8,
    COALESCE($9::boolean, FALSE),
    $10,
    $11,
    $12,
    $13,
    $14,
    $6::timestamptz,
    COALESCE($15::uuid, gen_random_uuid())
WHERE lock_task_owner_rows($1, NULL, $2)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, squad_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir"#
    )
        .bind(agent_id)
        .bind(runtime_id)
        .bind(priority)
        .bind(chat_session_id)
        .bind(initiator_user_id)
        .bind(fire_at)
        .bind(originator_user_id)
        .bind(accountable_user_id)
        .bind(force_fresh_session)
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
        squad_id: row.try_get(29)?,
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
    }))
}

pub async fn create_mika_onboarding_opening(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    content: &str,
    kickoff_created_at: Option<DateTime<Utc>>,
    id: Uuid,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"INSERT INTO chat_message (chat_session_id, role, content, message_kind, created_at, id)
VALUES (
    $1,
    'assistant',
    $2,
    'onboarding_opening',
    $3::timestamptz + interval '1 microsecond',
    COALESCE($4::uuid, gen_random_uuid())
)
RETURNING id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions"#
    )
        .bind(chat_session_id)
        .bind(content)
        .bind(kickoff_created_at)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn defer_chat_task_for_sealed_pending_media(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue AS task
SET status = 'deferred', fire_at = pending.max_until
FROM (
    SELECT max(message.channel_media_pending_until) AS max_until
    FROM chat_message AS message
    WHERE message.task_id = $1
      AND message.role = 'user'
      AND message.channel_media_pending_until > now()
) AS pending
WHERE task.id = $1
  AND pending.max_until IS NOT NULL
  AND (task.fire_at IS NULL OR task.fire_at < pending.max_until)
RETURNING task.id, task.agent_id, task.issue_id, task.status, task.priority, task.dispatched_at, task.started_at, task.completed_at, task.result, task.error, task.created_at, task.context, task.runtime_id, task.session_id, task.work_dir, task.trigger_comment_id, task.chat_session_id, task.autopilot_run_id, task.attempt, task.max_attempts, task.parent_task_id, task.failure_reason, task.trigger_summary, task.force_fresh_session, task.is_leader_task, task.wait_reason, task.initiator_user_id, task.handoff_note, task.prepare_lease_expires_at, task.squad_id, task.runtime_mcp_overlay, task.escalation_for_task_id, task.fire_at, task.originator_user_id, task.runtime_connected_apps, task.coalesced_comment_ids, task.delivered_comment_ids, task.chat_input_task_id, task.chat_finalize_deferred_at, task.originator_source, task.delegated_from_task_id, task.retry_of_task_id, task.rerun_of_task_id, task.rule_version_id, task.trigger_evidence_kind, task.trigger_evidence_ref_id, task.accountable_user_id, task.session_rollout_missing, task.retired_session_id, task.quick_actions_disabled, task.regenerate_quick_actions_for, task.branch_name, task.durable_work_dir"#
    )
        .bind(task_id)
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
        squad_id: row.try_get(29)?,
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
    }))
}

pub async fn delete_chat_draft_restore(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM chat_draft_restore
WHERE id = $1 AND chat_session_id = $2"#,
    )
    .bind(id)
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_chat_draft_restores_by_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM chat_draft_restore
WHERE chat_session_id = $1"#,
    )
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_chat_draft_restores_by_system_runtime_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM chat_draft_restore
WHERE chat_session_id IN (
    SELECT cs.id FROM chat_session cs
    JOIN agent a ON a.id = cs.agent_id
    WHERE a.runtime_id = $1 AND a.kind = 'system'
)"#,
    )
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_chat_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM chat_session WHERE id = $1 AND workspace_id = $2"#)
        .bind(id)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_user_chat_message_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"DELETE FROM chat_message
WHERE task_id = $1
  AND role = 'user'
  AND message_kind <> 'onboarding_kickoff'
RETURNING id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions"#
    )
        .bind(task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn get_channel_media_pending_until(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<Option<DateTime<Utc>>>> {
    let row = sqlx::query(
        r#"SELECT channel_media_pending_until
FROM chat_message
WHERE chat_session_id = $1
  AND role = 'user'
  AND message_kind != 'channel_command'
  AND channel_media_pending_until > now()
ORDER BY channel_media_pending_until DESC
LIMIT 1"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn get_chat_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions FROM chat_message
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn get_chat_message_by_task_assistant(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions FROM chat_message
WHERE task_id = $1 AND role = 'assistant'
ORDER BY created_at DESC
LIMIT 1"#
    )
        .bind(task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn get_chat_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id FROM chat_session
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn get_chat_session_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id FROM chat_session
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetLastChatTaskSessionRow {
    pub session_id: Option<String>,
    pub work_dir: Option<String>,
    pub runtime_id: Option<Uuid>,
}

pub async fn get_last_chat_task_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<GetLastChatTaskSessionRow>> {
    let row = sqlx::query(
        r#"WITH retired_sessions AS (
    SELECT DISTINCT r.retired_session_id AS session_id
    FROM agent_task_queue r
    WHERE r.chat_session_id = $1
      AND r.retired_session_id IS NOT NULL
), resume_overflow_at AS (
    -- completed_at alone, where the issue-side twin coalesces four columns:
    -- this query already selects and orders by bare completed_at throughout,
    -- so the cutoff has to be measured on the same clock as the values it is
    -- compared against. Change both halves together if that ever moves.
    SELECT MAX(t.completed_at) AS at
    FROM agent_task_queue t
    WHERE t.chat_session_id = $1
      AND t.status = 'failed'
      AND (
        COALESCE(t.failure_reason, '') = 'codex_resume_oversized'
        OR (COALESCE(t.error, '') ILIKE '%thread/resume failed%' AND COALESCE(t.error, '') ILIKE '%token too long%')
      )
), latest_per_session AS (
    SELECT DISTINCT ON (t.session_id)
        t.session_id, t.work_dir, t.runtime_id, t.status, t.failure_reason, t.error, t.completed_at
    FROM agent_task_queue t
    WHERE t.chat_session_id = $1
      AND t.session_id IS NOT NULL
      AND t.status IN ('completed', 'failed', 'cancelled')
    ORDER BY t.session_id, t.completed_at DESC
)
SELECT session_id, work_dir, runtime_id FROM latest_per_session
WHERE session_id NOT IN (SELECT session_id FROM retired_sessions)
  AND (
    status IN ('completed', 'cancelled')
    OR (
      status = 'failed'
      AND COALESCE(failure_reason, '') NOT IN ('iteration_limit', 'agent_fallback_message', 'api_invalid_request', 'codex_semantic_inactivity', 'agent_error.context_overflow', 'codex_resume_oversized')
      AND NOT (COALESCE(error, '') ILIKE '%400%' AND COALESCE(error, '') ILIKE '%invalid_request_error%')
      -- Mirrors the GetLastTaskSession auth-resolution guard: a provider that
      -- cannot resolve its auth method fails deterministically on resume, and
      -- the classification is agent_error.unknown (resume-safe), so only this
      -- text guard keeps the dead session from being replayed. This and
      -- GetLastTaskSession must move together.
      -- Keep in sync with ResumeUnsafeFailure and GetLastTaskSession.
      -- The phrase itself lives in taskfailure.AuthMethodUnresolved, which the
      -- daemon's in-turn fresh-session retry reads (GH #6777). This guard stays
      -- because it is the only protection for rows an older daemon wrote.
      AND NOT (COALESCE(error, '') ILIKE '%could not resolve authentication method%')
      AND NOT (COALESCE(error, '') ~* 'must not be empty|must be non-?empty|must have non-?empty|non-?empty content|cannot be empty|should not be empty'
               AND COALESCE(error, '') ~* 'role[^a-z0-9]{0,2}assistant|assistant message|message at position|messages\.[0-9]|messages\[[0-9]')
    )
  )
  -- PB-5722, mirroring GetLastTaskSession: an overflowed resume records no
  -- session, so exclude by time instead of by matching the failed row. Note
  -- this only guards the FALLBACK — the claim handler reads
  -- chat_session.session_id first, so a pointer still naming the oversized
  -- thread has to be cleared at fail time (see FailTask) to be covered.
  AND (
    (SELECT at FROM resume_overflow_at) IS NULL
    OR completed_at > (SELECT at FROM resume_overflow_at)
  )
ORDER BY completed_at DESC
LIMIT 1"#
    )
        .bind(chat_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetLastChatTaskSessionRow {
        session_id: row.try_get(0)?,
        work_dir: row.try_get(1)?,
        runtime_id: row.try_get(2)?,
    }))
}

pub async fn get_latest_assistant_chat_message_for_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions FROM chat_message
WHERE chat_session_id = $1 AND role = 'assistant' AND task_id IS NOT NULL
ORDER BY created_at DESC
LIMIT 1"#
    )
        .bind(chat_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn get_oldest_active_chat_session_for_creator_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id FROM chat_session
WHERE workspace_id = $1
  AND creator_id = $2
  AND agent_id = $3
  AND status = 'active'
ORDER BY created_at ASC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(creator_id)
        .bind(agent_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetPendingChatTaskRow {
    pub id: Option<Uuid>,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
}

pub async fn get_pending_chat_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<GetPendingChatTaskRow>> {
    let row = sqlx::query(
        r#"SELECT id, status, created_at FROM agent_task_queue
WHERE chat_session_id = $1 AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
  -- Background quick-actions regeneration passes are invisible to the chat UI:
  -- they own no assistant turn and must not raise the StatusPill or disable the
  -- composer (PB-5149 refresh follow-up).
  AND regenerate_quick_actions_for IS NULL
ORDER BY created_at DESC
LIMIT 1"#
    )
        .bind(chat_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetPendingChatTaskRow {
        id: row.try_get(0)?,
        status: row.try_get(1)?,
        created_at: row.try_get(2)?,
    }))
}

pub async fn get_public_chat_session_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"SELECT cs.id, cs.workspace_id, cs.agent_id, cs.creator_id, cs.title, cs.session_id, cs.work_dir, cs.status, cs.created_at, cs.updated_at, cs.unread_since, cs.runtime_id, cs.last_read_at, cs.is_agent_intro, cs.pinned_at, cs.project_id FROM chat_session AS cs
WHERE cs.id = $1
  AND cs.workspace_id = $2
  AND (
    EXISTS (
      SELECT 1 FROM chat_message AS public_message
      WHERE public_message.chat_session_id = cs.id
        AND public_message.message_kind != 'channel_command'
    )
    OR (
      NOT EXISTS (
        SELECT 1 FROM channel_chat_session_binding AS binding
        WHERE binding.chat_session_id = cs.id
      )
      AND NOT EXISTS (
        SELECT 1 FROM chat_message AS channel_message
        WHERE channel_message.chat_session_id = cs.id
          AND channel_message.channel_ingested
      )
    )
  )"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn has_active_chat_task_for_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
  SELECT 1 FROM agent_task_queue
  WHERE chat_session_id = $1
    AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
) AS has_active"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_pending_chat_tasks_by_creator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
  SELECT 1
  FROM agent_task_queue atq
  JOIN chat_session cs ON cs.id = atq.chat_session_id
  WHERE atq.chat_session_id IS NOT NULL
    AND atq.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
    -- Background quick-actions regeneration passes own no visible turn and must
    -- never light the FAB "running" indicator (PB-5149 refresh follow-up).
    AND atq.regenerate_quick_actions_for IS NULL
    AND cs.workspace_id = $1
    AND cs.creator_id = $2
    AND cs.agent_id = ANY($3::uuid[])
) AS has_pending"#,
    )
    .bind(workspace_id)
    .bind(creator_id)
    .bind(agent_ids)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_pending_chat_turn_for_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
  SELECT 1 FROM agent_task_queue
  WHERE chat_session_id = $1
    AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
    AND regenerate_quick_actions_for IS NULL
) AS has_pending"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn link_chat_message_to_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message
SET task_id = $2
WHERE id = $1 AND role = 'user'"#,
    )
    .bind(id)
    .bind(task_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn link_unowned_channel_chat_messages_to_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message AS message
SET task_id = $1
WHERE message.chat_session_id = $2
  AND message.role = 'user'
  AND message.task_id IS NULL
  AND message.message_kind != 'channel_command'
  AND NOT EXISTS (
      SELECT 1
      FROM chat_message AS prior
      LEFT JOIN agent_task_queue AS prior_turn
        ON prior_turn.id = prior.task_id
      LEFT JOIN agent_task_queue AS prior_batch
        ON prior_batch.id = COALESCE(prior_turn.chat_input_task_id, prior_turn.id)
      WHERE prior.chat_session_id = $2
        AND prior.role != 'user'
        AND (prior.created_at, prior.id) > (message.created_at, message.id)
        AND (prior_batch.id IS NULL OR prior_batch.created_at > message.created_at)
  )"#,
    )
    .bind(task_id)
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAgentBuilderSessionsByCreatorRow {
    pub id: Option<Uuid>,
    pub title: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub runtime_id: Option<Uuid>,
    pub last_message_content: String,
    pub last_message_role: String,
    pub last_message_at: Option<DateTime<Utc>>,
    pub stored_draft: Option<serde_json::Value>,
}

pub async fn list_agent_builder_sessions_by_creator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
) -> anyhow::Result<Vec<ListAgentBuilderSessionsByCreatorRow>> {
    let rows = sqlx::query(
        r#"SELECT cs.id,
       cs.title,
       cs.created_at,
       cs.updated_at,
       a.runtime_id,
       COALESCE(lm.content, '') AS last_message_content,
       COALESCE(lm.role, '') AS last_message_role,
       lm.created_at AS last_message_at,
       d.draft AS stored_draft
FROM chat_session cs
JOIN agent a ON a.id = cs.agent_id
LEFT JOIN agent_builder_draft d ON d.chat_session_id = cs.id
LEFT JOIN LATERAL (
  SELECT content, role, created_at
    FROM chat_message m
   WHERE m.chat_session_id = cs.id
   ORDER BY m.created_at DESC
   LIMIT 1
) lm ON true
WHERE cs.workspace_id = $1
  AND cs.creator_id = $2
  AND cs.status = 'active'
  AND a.kind = 'system'
  AND a.system_key LIKE 'agent_builder:%'
  AND (lm.created_at IS NOT NULL OR d.chat_session_id IS NOT NULL)
ORDER BY COALESCE(lm.created_at, d.updated_at, cs.updated_at) DESC"#,
    )
    .bind(workspace_id)
    .bind(creator_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAgentBuilderSessionsByCreatorRow {
            id: row.try_get(0)?,
            title: row.try_get(1)?,
            created_at: row.try_get(2)?,
            updated_at: row.try_get(3)?,
            runtime_id: row.try_get(4)?,
            last_message_content: row.try_get(5)?,
            last_message_role: row.try_get(6)?,
            last_message_at: row.try_get(7)?,
            stored_draft: row.try_get(8)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAllChatSessionsByCreatorRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub creator_id: Option<Uuid>,
    pub title: String,
    pub session_id: Option<String>,
    pub work_dir: Option<String>,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub unread_since: Option<DateTime<Utc>>,
    pub runtime_id: Option<Uuid>,
    pub last_read_at: Option<DateTime<Utc>>,
    pub is_agent_intro: bool,
    pub pinned_at: Option<DateTime<Utc>>,
    pub project_id: Option<Uuid>,
    pub unread_count: i32,
    pub last_message_content: String,
    pub last_message_role: String,
    pub last_message_at: Option<DateTime<Utc>>,
    pub last_message_failure_reason: Option<String>,
    pub last_message_kind: String,
}

pub async fn list_all_chat_sessions_by_creator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
) -> anyhow::Result<Vec<ListAllChatSessionsByCreatorRow>> {
    let rows = sqlx::query(
        r#"SELECT cs.id, cs.workspace_id, cs.agent_id, cs.creator_id, cs.title, cs.session_id, cs.work_dir, cs.status, cs.created_at, cs.updated_at, cs.unread_since, cs.runtime_id, cs.last_read_at, cs.is_agent_intro, cs.pinned_at, cs.project_id,
       CASE WHEN cs.status = 'archived' THEN 0
            ELSE (SELECT count(*) FROM chat_message m
                    WHERE m.chat_session_id = cs.id
                      AND m.role = 'assistant'
                      AND m.created_at > cs.last_read_at)
       END::int AS unread_count,
       COALESCE(lm.content, '') AS last_message_content,
       COALESCE(lm.role, '') AS last_message_role,
       lm.created_at AS last_message_at,
       lm.failure_reason AS last_message_failure_reason,
       COALESCE(lm.message_kind, '') AS last_message_kind
FROM chat_session cs
LEFT JOIN LATERAL (
  SELECT content, role, created_at, failure_reason, message_kind
    FROM chat_message m
   WHERE m.chat_session_id = cs.id
     AND m.message_kind != 'channel_command'
   ORDER BY m.created_at DESC
   LIMIT 1
) lm ON true
WHERE cs.workspace_id = $1 AND cs.creator_id = $2
  AND (
    lm.created_at IS NOT NULL
    OR (
      NOT EXISTS (
        SELECT 1 FROM channel_chat_session_binding AS binding
        WHERE binding.chat_session_id = cs.id
      )
      AND NOT EXISTS (
        SELECT 1 FROM chat_message AS channel_message
        WHERE channel_message.chat_session_id = cs.id
          AND channel_message.channel_ingested
      )
    )
  )
ORDER BY (cs.pinned_at IS NOT NULL) DESC, cs.pinned_at DESC, COALESCE(lm.created_at, cs.updated_at) DESC"#
    )
        .bind(workspace_id)
        .bind(creator_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAllChatSessionsByCreatorRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            agent_id: row.try_get(2)?,
            creator_id: row.try_get(3)?,
            title: row.try_get(4)?,
            session_id: row.try_get(5)?,
            work_dir: row.try_get(6)?,
            status: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
            unread_since: row.try_get(10)?,
            runtime_id: row.try_get(11)?,
            last_read_at: row.try_get(12)?,
            is_agent_intro: row.try_get(13)?,
            pinned_at: row.try_get(14)?,
            project_id: row.try_get(15)?,
            unread_count: row.try_get(16)?,
            last_message_content: row.try_get(17)?,
            last_message_role: row.try_get(18)?,
            last_message_at: row.try_get(19)?,
            last_message_failure_reason: row.try_get(20)?,
            last_message_kind: row.try_get(21)?,
        });
    }
    Ok(out)
}

pub async fn list_chat_draft_restores_by_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<ChatDraftRestore>> {
    let rows = sqlx::query(
        r#"SELECT id, chat_session_id, task_id, content, attachment_ids, created_at FROM chat_draft_restore
WHERE chat_session_id = $1
ORDER BY created_at ASC"#
    )
        .bind(chat_session_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChatDraftRestore {
            id: row.try_get(0)?,
            chat_session_id: row.try_get(1)?,
            task_id: row.try_get(2)?,
            content: row.try_get(3)?,
            attachment_ids: row.try_get(4)?,
            created_at: row.try_get(5)?,
        });
    }
    Ok(out)
}

pub async fn list_chat_input_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query(
        r#"SELECT id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions FROM chat_message
WHERE task_id = $1 AND role = 'user'
ORDER BY created_at ASC, id ASC"#
    )
        .bind(task_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChatMessage {
            id: row.try_get(0)?,
            chat_session_id: row.try_get(1)?,
            role: row.try_get(2)?,
            content: row.try_get(3)?,
            task_id: row.try_get(4)?,
            created_at: row.try_get(5)?,
            failure_reason: row.try_get(6)?,
            elapsed_ms: row.try_get(7)?,
            message_kind: row.try_get(8)?,
            channel_media_pending_until: row.try_get(9)?,
            channel_ingested: row.try_get(10)?,
            quick_actions: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_chat_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query(
        r#"SELECT message.id, message.chat_session_id, message.role, message.content, message.task_id, message.created_at, message.failure_reason, message.elapsed_ms, message.message_kind, message.channel_media_pending_until, message.channel_ingested, message.quick_actions FROM chat_message AS message
WHERE message.chat_session_id = $1
  AND message.message_kind != 'channel_command'
  AND NOT (
    message.role = 'user'
    AND EXISTS (
      SELECT 1
      FROM agent_task_queue AS task
      WHERE task.chat_session_id = message.chat_session_id
        AND task.status = 'queued'
        AND task.id = message.task_id
        -- "Queued follow-up" is positional, not the row's transient status:
        -- the first pending task is the current turn even before claim.
        AND task.id <> (
          SELECT head.id
          FROM agent_task_queue AS head
          WHERE head.chat_session_id = $1
            AND head.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
            AND head.regenerate_quick_actions_for IS NULL
          ORDER BY
            CASE
              WHEN head.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
              WHEN head.status = 'deferred' THEN 1
              ELSE 2
            END,
            head.priority DESC,
            head.created_at ASC,
            head.id ASC
          LIMIT 1
        )
    )
  )
ORDER BY message.created_at ASC, message.id ASC"#
    )
        .bind(chat_session_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChatMessage {
            id: row.try_get(0)?,
            chat_session_id: row.try_get(1)?,
            role: row.try_get(2)?,
            content: row.try_get(3)?,
            task_id: row.try_get(4)?,
            created_at: row.try_get(5)?,
            failure_reason: row.try_get(6)?,
            elapsed_ms: row.try_get(7)?,
            message_kind: row.try_get(8)?,
            channel_media_pending_until: row.try_get(9)?,
            channel_ingested: row.try_get(10)?,
            quick_actions: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_chat_messages_for_legacy_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query(
        r#"SELECT message.id, message.chat_session_id, message.role, message.content, message.task_id, message.created_at, message.failure_reason, message.elapsed_ms, message.message_kind, message.channel_media_pending_until, message.channel_ingested, message.quick_actions FROM chat_message AS message
WHERE message.chat_session_id = $1
  AND NOT (
    message.role = 'user'
    AND EXISTS (
      SELECT 1
      FROM agent_task_queue AS task
      WHERE task.chat_session_id = message.chat_session_id
        AND task.status = 'queued'
        AND task.id = message.task_id
        AND task.id <> (
          SELECT head.id
          FROM agent_task_queue AS head
          WHERE head.chat_session_id = $1
            AND head.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
            AND head.regenerate_quick_actions_for IS NULL
          ORDER BY
            CASE
              WHEN head.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
              WHEN head.status = 'deferred' THEN 1
              ELSE 2
            END,
            head.priority DESC,
            head.created_at ASC,
            head.id ASC
          LIMIT 1
        )
    )
  )
ORDER BY message.created_at ASC, message.id ASC"#
    )
        .bind(chat_session_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChatMessage {
            id: row.try_get(0)?,
            chat_session_id: row.try_get(1)?,
            role: row.try_get(2)?,
            content: row.try_get(3)?,
            task_id: row.try_get(4)?,
            created_at: row.try_get(5)?,
            failure_reason: row.try_get(6)?,
            elapsed_ms: row.try_get(7)?,
            message_kind: row.try_get(8)?,
            channel_media_pending_until: row.try_get(9)?,
            channel_ingested: row.try_get(10)?,
            quick_actions: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_chat_messages_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    limit: i32,
    before_created_at: Option<DateTime<Utc>>,
    before_id: Uuid,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = sqlx::query(
        r#"SELECT message.id, message.chat_session_id, message.role, message.content, message.task_id, message.created_at, message.failure_reason, message.elapsed_ms, message.message_kind, message.channel_media_pending_until, message.channel_ingested, message.quick_actions FROM chat_message AS message
WHERE message.chat_session_id = $1
  AND message.message_kind != 'channel_command'
  AND NOT (
    message.role = 'user'
    AND EXISTS (
      SELECT 1
      FROM agent_task_queue AS task
      WHERE task.chat_session_id = message.chat_session_id
        AND task.status = 'queued'
        AND task.id = message.task_id
        AND task.id <> (
          SELECT head.id
          FROM agent_task_queue AS head
          WHERE head.chat_session_id = $1
            AND head.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
            AND head.regenerate_quick_actions_for IS NULL
          ORDER BY
            CASE
              WHEN head.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
              WHEN head.status = 'deferred' THEN 1
              ELSE 2
            END,
            head.priority DESC,
            head.created_at ASC,
            head.id ASC
          LIMIT 1
        )
    )
  )
  AND (
    $3::timestamptz IS NULL
    OR (message.created_at, message.id) < ($3::timestamptz, $4::uuid)
  )
ORDER BY message.created_at DESC, message.id DESC
LIMIT $2"#
    )
        .bind(chat_session_id)
        .bind(limit)
        .bind(before_created_at)
        .bind(before_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChatMessage {
            id: row.try_get(0)?,
            chat_session_id: row.try_get(1)?,
            role: row.try_get(2)?,
            content: row.try_get(3)?,
            task_id: row.try_get(4)?,
            created_at: row.try_get(5)?,
            failure_reason: row.try_get(6)?,
            elapsed_ms: row.try_get(7)?,
            message_kind: row.try_get(8)?,
            channel_media_pending_until: row.try_get(9)?,
            channel_ingested: row.try_get(10)?,
            quick_actions: row.try_get(11)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListChatSessionsByCreatorRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub creator_id: Option<Uuid>,
    pub title: String,
    pub session_id: Option<String>,
    pub work_dir: Option<String>,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub unread_since: Option<DateTime<Utc>>,
    pub runtime_id: Option<Uuid>,
    pub last_read_at: Option<DateTime<Utc>>,
    pub is_agent_intro: bool,
    pub pinned_at: Option<DateTime<Utc>>,
    pub project_id: Option<Uuid>,
    pub unread_count: i32,
    pub last_message_content: String,
    pub last_message_role: String,
    pub last_message_at: Option<DateTime<Utc>>,
    pub last_message_failure_reason: Option<String>,
    pub last_message_kind: String,
}

pub async fn list_chat_sessions_by_creator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
) -> anyhow::Result<Vec<ListChatSessionsByCreatorRow>> {
    let rows = sqlx::query(
        r#"SELECT cs.id, cs.workspace_id, cs.agent_id, cs.creator_id, cs.title, cs.session_id, cs.work_dir, cs.status, cs.created_at, cs.updated_at, cs.unread_since, cs.runtime_id, cs.last_read_at, cs.is_agent_intro, cs.pinned_at, cs.project_id,
       (SELECT count(*) FROM chat_message m
          WHERE m.chat_session_id = cs.id
            AND m.role = 'assistant'
            AND m.created_at > cs.last_read_at)::int AS unread_count,
       COALESCE(lm.content, '') AS last_message_content,
       COALESCE(lm.role, '') AS last_message_role,
       lm.created_at AS last_message_at,
       lm.failure_reason AS last_message_failure_reason,
       COALESCE(lm.message_kind, '') AS last_message_kind
FROM chat_session cs
LEFT JOIN LATERAL (
  SELECT content, role, created_at, failure_reason, message_kind
    FROM chat_message m
   WHERE m.chat_session_id = cs.id
     AND m.message_kind != 'channel_command'
   ORDER BY m.created_at DESC
   LIMIT 1
) lm ON true
WHERE cs.workspace_id = $1 AND cs.creator_id = $2 AND cs.status = 'active'
  AND (
    lm.created_at IS NOT NULL
    OR (
      NOT EXISTS (
        SELECT 1 FROM channel_chat_session_binding AS binding
        WHERE binding.chat_session_id = cs.id
      )
      AND NOT EXISTS (
        SELECT 1 FROM chat_message AS channel_message
        WHERE channel_message.chat_session_id = cs.id
          AND channel_message.channel_ingested
      )
    )
  )
ORDER BY (cs.pinned_at IS NOT NULL) DESC, cs.pinned_at DESC, COALESCE(lm.created_at, cs.updated_at) DESC"#
    )
        .bind(workspace_id)
        .bind(creator_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListChatSessionsByCreatorRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            agent_id: row.try_get(2)?,
            creator_id: row.try_get(3)?,
            title: row.try_get(4)?,
            session_id: row.try_get(5)?,
            work_dir: row.try_get(6)?,
            status: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
            unread_since: row.try_get(10)?,
            runtime_id: row.try_get(11)?,
            last_read_at: row.try_get(12)?,
            is_agent_intro: row.try_get(13)?,
            pinned_at: row.try_get(14)?,
            project_id: row.try_get(15)?,
            unread_count: row.try_get(16)?,
            last_message_content: row.try_get(17)?,
            last_message_role: row.try_get(18)?,
            last_message_at: row.try_get(19)?,
            last_message_failure_reason: row.try_get(20)?,
            last_message_kind: row.try_get(21)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListPendingChatTasksByCreatorRow {
    pub task_id: Option<Uuid>,
    pub status: String,
    pub chat_session_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
}

pub async fn list_pending_chat_tasks_by_creator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    creator_id: Uuid,
) -> anyhow::Result<Vec<ListPendingChatTasksByCreatorRow>> {
    let rows = sqlx::query(
        r#"SELECT atq.id AS task_id, atq.status, atq.chat_session_id, cs.agent_id
FROM agent_task_queue atq
JOIN chat_session cs ON cs.id = atq.chat_session_id
WHERE atq.chat_session_id IS NOT NULL
  AND atq.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  -- Exclude background quick-actions regeneration passes: they own no assistant
  -- turn and must not surface as "running" chat work (PB-5149 refresh follow-up).
  AND atq.regenerate_quick_actions_for IS NULL
  AND cs.workspace_id = $1
  AND cs.creator_id = $2
ORDER BY atq.created_at DESC"#,
    )
    .bind(workspace_id)
    .bind(creator_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPendingChatTasksByCreatorRow {
            task_id: row.try_get(0)?,
            status: row.try_get(1)?,
            chat_session_id: row.try_get(2)?,
            agent_id: row.try_get(3)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListPendingChatTasksForSessionRow {
    pub id: Option<Uuid>,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub message_id: Option<Uuid>,
    pub content: String,
}

pub async fn list_pending_chat_tasks_for_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<ListPendingChatTasksForSessionRow>> {
    let rows = sqlx::query(
        r#"SELECT
    task.id,
    task.status,
    task.created_at,
    message.id AS message_id,
    COALESCE(message.content, '')::text AS content
FROM agent_task_queue AS task
LEFT JOIN LATERAL (
    SELECT input.id, input.content
    FROM chat_message AS input
    WHERE input.task_id = COALESCE(task.chat_input_task_id, task.id)
      AND input.role = 'user'
    ORDER BY input.created_at ASC, input.id ASC
    LIMIT 1
) AS message ON TRUE
WHERE task.chat_session_id = $1
  AND task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  AND task.regenerate_quick_actions_for IS NULL
ORDER BY
    CASE
      WHEN task.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
      WHEN task.status = 'deferred' THEN 1
      ELSE 2
    END,
    task.priority DESC,
    task.created_at ASC,
    task.id ASC"#,
    )
    .bind(chat_session_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPendingChatTasksForSessionRow {
            id: row.try_get(0)?,
            status: row.try_get(1)?,
            created_at: row.try_get(2)?,
            message_id: row.try_get(3)?,
            content: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn lock_chat_session_for_append(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM chat_session
WHERE id = $1
FOR KEY SHARE"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_chat_session_for_delete(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM chat_session
WHERE id = $1
FOR UPDATE"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_chat_session_for_draft_write(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id FROM chat_session
WHERE id = $1
FOR UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn lock_chat_session_for_enqueue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id FROM chat_session
WHERE id = $1
FOR NO KEY UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn lock_chat_session_for_runtime_bind(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM chat_session
WHERE id = $1
FOR UPDATE"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_chat_session_for_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT cs.id
FROM agent_task_queue t
JOIN chat_session cs ON cs.id = t.chat_session_id
WHERE t.id = $1
FOR UPDATE OF cs"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_chat_sessions_by_system_runtime_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT cs.id FROM chat_session cs
JOIN agent a ON a.id = cs.agent_id
WHERE a.runtime_id = $1 AND a.kind = 'system'
ORDER BY cs.id
FOR UPDATE OF cs"#,
    )
    .bind(runtime_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn lock_chat_sessions_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM chat_session
WHERE workspace_id = $1
ORDER BY id
FOR UPDATE"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn mark_chat_session_read(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_session SET last_read_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrioritizeQueuedChatTaskRow {
    pub task_id: Option<Uuid>,
    pub active_task_id: Option<Uuid>,
}

pub async fn prioritize_queued_chat_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<PrioritizeQueuedChatTaskRow>> {
    let row = sqlx::query(
        r#"WITH target AS MATERIALIZED (
  SELECT candidate.id
  FROM agent_task_queue AS candidate
  WHERE candidate.id = $2
    AND candidate.chat_session_id = $1
    AND candidate.status = 'queued'
    -- "Send now" is valid only while there is a visible claimed task for the
    -- client to cancel. If the visible head is still queued (or deferred), the
    -- selected row would otherwise replace it without any active_task_id.
    AND EXISTS (
      SELECT 1
      FROM agent_task_queue AS active
      WHERE active.chat_session_id = $1
        AND active.status IN ('dispatched', 'running', 'waiting_local_directory')
        AND active.regenerate_quick_actions_for IS NULL
    )
  FOR UPDATE
), demoted AS (
  UPDATE agent_task_queue AS queued
  SET priority = 3
  WHERE queued.chat_session_id = $1
    AND queued.id <> $2
    AND queued.status = 'queued'
    AND queued.priority >= 4
    AND EXISTS (SELECT 1 FROM target)
), prioritized AS (
  UPDATE agent_task_queue AS selected
  SET priority = 4
  FROM target
  WHERE selected.id = target.id
  RETURNING selected.id
)
SELECT
  prioritized.id AS task_id,
  (
    SELECT active.id
    FROM agent_task_queue AS active
    WHERE active.chat_session_id = $1
      AND active.status IN ('dispatched', 'running', 'waiting_local_directory')
      AND active.regenerate_quick_actions_for IS NULL
    ORDER BY active.created_at ASC, active.id ASC
    LIMIT 1
  )::uuid AS active_task_id
FROM prioritized"#,
    )
    .bind(chat_session_id)
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PrioritizeQueuedChatTaskRow {
        task_id: row.try_get(0)?,
        active_task_id: row.try_get(1)?,
    }))
}

pub async fn promote_channel_chat_tasks_if_media_ready(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Vec<AgentTaskQueue>> {
    let rows = sqlx::query(
        r#"UPDATE agent_task_queue AS task
SET status = 'queued', fire_at = NULL
WHERE task.chat_session_id = $1
  AND task.status = 'deferred'
  AND task.issue_id IS NULL
  AND task.parent_task_id IS NULL
  AND task.escalation_for_task_id IS NULL
  AND NOT EXISTS (
      SELECT 1
      FROM chat_message AS message
      WHERE message.chat_session_id = $1
        AND message.role = 'user'
        AND message.message_kind != 'channel_command'
        AND message.channel_media_pending_until > now()
  )
RETURNING task.id, task.agent_id, task.issue_id, task.status, task.priority, task.dispatched_at, task.started_at, task.completed_at, task.result, task.error, task.created_at, task.context, task.runtime_id, task.session_id, task.work_dir, task.trigger_comment_id, task.chat_session_id, task.autopilot_run_id, task.attempt, task.max_attempts, task.parent_task_id, task.failure_reason, task.trigger_summary, task.force_fresh_session, task.is_leader_task, task.wait_reason, task.initiator_user_id, task.handoff_note, task.prepare_lease_expires_at, task.squad_id, task.runtime_mcp_overlay, task.escalation_for_task_id, task.fire_at, task.originator_user_id, task.runtime_connected_apps, task.coalesced_comment_ids, task.delivered_comment_ids, task.chat_input_task_id, task.chat_finalize_deferred_at, task.originator_source, task.delegated_from_task_id, task.retry_of_task_id, task.rerun_of_task_id, task.rule_version_id, task.trigger_evidence_kind, task.trigger_evidence_ref_id, task.accountable_user_id, task.session_rollout_missing, task.retired_session_id, task.quick_actions_disabled, task.regenerate_quick_actions_for, task.branch_name, task.durable_work_dir"#
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
            squad_id: row.try_get(29)?,
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
        });
    }
    Ok(out)
}

pub async fn reanchor_claimed_direct_chat_input(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    dispatched_at: Option<DateTime<Utc>>,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH latest_visible AS (
    SELECT
        claimed_input.id AS claimed_input_id,
        claimed_input.created_at AS input_created_at,
        prior.created_at AS prior_created_at
    FROM chat_message AS claimed_input
    CROSS JOIN LATERAL (
        SELECT prior.created_at
        FROM chat_message AS prior
        WHERE prior.chat_session_id = claimed_input.chat_session_id
          AND prior.id != claimed_input.id
          AND NOT (
            prior.role = 'user'
            AND EXISTS (
              SELECT 1
              FROM agent_task_queue AS queued_task
              WHERE queued_task.chat_session_id = prior.chat_session_id
                AND queued_task.status = 'queued'
                AND queued_task.id = prior.task_id
                AND queued_task.id <> (
                  SELECT head.id
                  FROM agent_task_queue AS head
                  WHERE head.chat_session_id = claimed_input.chat_session_id
                    AND head.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
                    AND head.regenerate_quick_actions_for IS NULL
                  ORDER BY
                    CASE
                      WHEN head.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
                      WHEN head.status = 'deferred' THEN 1
                      ELSE 2
                    END,
                    head.priority DESC,
                    head.created_at ASC,
                    head.id ASC
                  LIMIT 1
                )
            )
          )
        ORDER BY prior.created_at DESC, prior.id DESC
        LIMIT 1
    ) AS prior
    WHERE claimed_input.task_id = $2
      AND claimed_input.role = 'user'
      AND NOT claimed_input.channel_ingested
      -- The adopted onboarding kickoff is never a visible row, so it has no
      -- turn boundary to correct — and reanchoring it would actively break the
      -- one thing its position controls. It is deliberately older than the
      -- member's message so the runtime reads "context, then their words";
      -- moving it to dispatch time reverses that, and because the batch's two
      -- rows would then share one timestamp, their order falls to random UUIDs
      -- (PB-5827).
      AND claimed_input.message_kind <> 'onboarding_kickoff'
)
UPDATE chat_message AS claimed_input
SET created_at = GREATEST(
    $1::timestamptz,
    latest_visible.prior_created_at + interval '1 microsecond'
)
FROM latest_visible
WHERE claimed_input.id = latest_visible.claimed_input_id
  AND latest_visible.prior_created_at >= latest_visible.input_created_at"#
    )
        .bind(dispatched_at)
        .bind(task_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn reanchor_next_queued_direct_chat_input(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    assistant_created_at: Option<DateTime<Utc>>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message AS queued_input
SET created_at = $2::timestamptz + interval '1 microsecond'
WHERE queued_input.chat_session_id = $1
  AND queued_input.role = 'user'
  AND NOT queued_input.channel_ingested
  -- Same exclusion, same reason as ReanchorClaimedDirectChatInput: the hidden
  -- kickoff has no visible position to fix, and moving it would put the
  -- product's context after the member's message inside one input batch.
  AND queued_input.message_kind <> 'onboarding_kickoff'
  AND queued_input.created_at <= $2::timestamptz
  AND EXISTS (
    SELECT 1
    FROM agent_task_queue AS queued_task
    WHERE queued_task.id = queued_input.task_id
      AND queued_task.chat_session_id = queued_input.chat_session_id
      AND queued_task.status = 'queued'
      AND queued_task.chat_input_task_id = queued_task.id
      AND queued_task.regenerate_quick_actions_for IS NULL
      AND queued_task.id = (
        SELECT head.id
        FROM agent_task_queue AS head
        WHERE head.chat_session_id = queued_input.chat_session_id
          AND head.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
          AND head.regenerate_quick_actions_for IS NULL
        ORDER BY
          CASE
            WHEN head.status IN ('dispatched', 'running', 'waiting_local_directory') THEN 0
            WHEN head.status = 'deferred' THEN 1
            ELSE 2
          END,
          head.priority DESC,
          head.created_at ASC,
          head.id ASC
        LIMIT 1
      )
  )"#
    )
        .bind(chat_session_id)
        .bind(assistant_created_at)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn release_onboarding_kickoff_from_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message
SET task_id = (
    SELECT successor.id
    FROM agent_task_queue AS successor
    WHERE successor.chat_session_id = chat_message.chat_session_id
      AND successor.status = 'queued'
      AND successor.chat_input_task_id = successor.id
      AND successor.regenerate_quick_actions_for IS NULL
      AND successor.id <> $1
    ORDER BY successor.priority DESC, successor.created_at ASC, successor.id ASC
    LIMIT 1
)
WHERE task_id = $1
  AND role = 'user'
  AND message_kind = 'onboarding_kickoff'"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_chat_message_quick_actions_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    quick_actions: &serde_json::Value,
) -> anyhow::Result<Option<ChatMessage>> {
    let row = sqlx::query(
        r#"UPDATE chat_message
SET quick_actions = $2
WHERE id = (
    SELECT inner_msg.id FROM chat_message AS inner_msg
    WHERE inner_msg.task_id = $1 AND inner_msg.role = 'assistant'
    ORDER BY inner_msg.created_at DESC
    LIMIT 1
)
RETURNING id, chat_session_id, role, content, task_id, created_at, failure_reason, elapsed_ms, message_kind, channel_media_pending_until, channel_ingested, quick_actions"#
    )
        .bind(task_id)
        .bind(quick_actions)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        role: row.try_get(2)?,
        content: row.try_get(3)?,
        task_id: row.try_get(4)?,
        created_at: row.try_get(5)?,
        failure_reason: row.try_get(6)?,
        elapsed_ms: row.try_get(7)?,
        message_kind: row.try_get(8)?,
        channel_media_pending_until: row.try_get(9)?,
        channel_ingested: row.try_get(10)?,
        quick_actions: row.try_get(11)?,
    }))
}

pub async fn set_chat_session_archived(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    archived: bool,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"UPDATE chat_session
SET status = CASE WHEN $2::bool THEN 'archived' ELSE 'active' END,
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id"#
    )
        .bind(id)
        .bind(archived)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn set_chat_session_pinned(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    pinned: bool,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"UPDATE chat_session
SET pinned_at = CASE WHEN $2::bool THEN COALESCE(pinned_at, now()) ELSE NULL END
WHERE id = $1
RETURNING id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id"#
    )
        .bind(id)
        .bind(pinned)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn set_chat_task_input_owner_self(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"UPDATE agent_task_queue
SET chat_input_task_id = id
WHERE id = $1
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, autopilot_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, squad_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir"#
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
        squad_id: row.try_get(29)?,
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
    }))
}

pub async fn task_has_channel_ingested_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
    SELECT 1 FROM chat_message
    WHERE task_id = $1
      AND role = 'user'
      AND channel_ingested
) AS channel_ingested"#,
    )
    .bind(task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn task_input_is_onboarding_kickoff_only(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<Option<bool>>> {
    let row = sqlx::query(
        r#"SELECT
    EXISTS (
        SELECT 1 FROM chat_message AS kickoff
        WHERE kickoff.task_id = $1
          AND kickoff.role = 'user'
          AND kickoff.message_kind = 'onboarding_kickoff'
    )
    AND NOT EXISTS (
        SELECT 1 FROM chat_message AS typed
        WHERE typed.task_id = $1
          AND typed.role = 'user'
          AND typed.message_kind <> 'onboarding_kickoff'
    ) AS kickoff_only"#,
    )
    .bind(task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn touch_chat_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_session SET updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_chat_message_content_for_channel_media(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    content: &str,
    id: Uuid,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_message
SET content = $1
WHERE id = $2
  AND chat_session_id = $3
  AND role = 'user'
  AND channel_ingested"#,
    )
    .bind(content)
    .bind(id)
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_chat_session_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_id: Uuid,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"UPDATE chat_session
SET project_id = $1
WHERE id = $2 AND workspace_id = $3
RETURNING id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id"#
    )
        .bind(project_id)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn update_chat_session_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    session_id: Option<&str>,
    work_dir: Option<&str>,
    runtime_id: Option<Uuid>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE chat_session
SET session_id = COALESCE($1, session_id),
    work_dir = COALESCE($2, work_dir),
    runtime_id = COALESCE($3, runtime_id),
    updated_at = now()
WHERE id = $4"#,
    )
    .bind(session_id)
    .bind(work_dir)
    .bind(runtime_id)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_chat_session_title(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    title: &str,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"UPDATE chat_session SET title = $2, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id"#
    )
        .bind(id)
        .bind(title)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}

pub async fn update_chat_session_title_if_current(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    new_title: &str,
    id: Uuid,
    expected_title: &str,
) -> anyhow::Result<Option<ChatSession>> {
    let row = sqlx::query(
        r#"UPDATE chat_session SET title = $1, updated_at = now()
WHERE id = $2 AND title = $3
RETURNING id, workspace_id, agent_id, creator_id, title, session_id, work_dir, status, created_at, updated_at, unread_since, runtime_id, last_read_at, is_agent_intro, pinned_at, project_id"#
    )
        .bind(new_title)
        .bind(id)
        .bind(expected_title)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChatSession {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        creator_id: row.try_get(3)?,
        title: row.try_get(4)?,
        session_id: row.try_get(5)?,
        work_dir: row.try_get(6)?,
        status: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        unread_since: row.try_get(10)?,
        runtime_id: row.try_get(11)?,
        last_read_at: row.try_get(12)?,
        is_agent_intro: row.try_get(13)?,
        pinned_at: row.try_get(14)?,
        project_id: row.try_get(15)?,
    }))
}
