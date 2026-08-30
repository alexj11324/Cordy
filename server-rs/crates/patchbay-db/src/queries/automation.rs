//! Typed SQL queries for automation records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn add_automation_collaborator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    user_type: &str,
    user_id: Uuid,
    granted_by: Uuid,
) -> anyhow::Result<Option<AutomationCollaborator>> {
    let row = sqlx::query(
        r#"INSERT INTO automation_collaborator (automation_id, user_type, user_id, granted_by)
VALUES ($1, $2, $3, $4)
ON CONFLICT (automation_id, user_type, user_id)
    DO UPDATE SET granted_by = EXCLUDED.granted_by
RETURNING automation_id, user_type, user_id, granted_by, created_at"#,
    )
    .bind(automation_id)
    .bind(user_type)
    .bind(user_id)
    .bind(granted_by)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationCollaborator {
        automation_id: row.try_get(0)?,
        user_type: row.try_get(1)?,
        user_id: row.try_get(2)?,
        granted_by: row.try_get(3)?,
        created_at: row.try_get(4)?,
    }))
}

pub async fn add_automation_subscriber(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    user_type: &str,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO automation_subscriber (automation_id, user_type, user_id)
VALUES ($1, $2, $3)
ON CONFLICT (automation_id, user_type, user_id) DO NOTHING"#,
    )
    .bind(automation_id)
    .bind(user_type)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn advance_trigger_next_run(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    next_run_at: Option<DateTime<Utc>>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE automation_trigger
SET next_run_at = $2,
    last_fired_at = now(),
    updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(next_run_at)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn archive_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE automation
SET status = 'archived', pause_reason = NULL, updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn create_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    title: &str,
    assignee_type: &str,
    assignee_id: Uuid,
    status: &str,
    execution_mode: &str,
    created_by_type: &str,
    created_by_id: Uuid,
    description: Option<&str>,
    issue_title_template: Option<&str>,
    project_id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"INSERT INTO automation (
    workspace_id, title, description, assignee_type, assignee_id,
    status, execution_mode, issue_title_template, project_id,
    created_by_type, created_by_id
) VALUES (
    $1, $2, $9, $3, $4,
    $5, $6, $10, $11,
    $7, $8
) RETURNING id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason"#
    )
        .bind(workspace_id)
        .bind(title)
        .bind(assignee_type)
        .bind(assignee_id)
        .bind(status)
        .bind(execution_mode)
        .bind(created_by_type)
        .bind(created_by_id)
        .bind(description)
        .bind(issue_title_template)
        .bind(project_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

pub async fn create_automation_rule_version(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    workspace_id: Uuid,
    published_by_type: &str,
    published_by_id: Option<Uuid>,
    config_summary: &serde_json::Value,
) -> anyhow::Result<Option<AutomationRuleVersion>> {
    let row = sqlx::query(
        r#"INSERT INTO automation_rule_version (
    automation_id, workspace_id, published_by_type, published_by_id, config_summary
)
VALUES (
    $1, $2, $3, $4,
    COALESCE($5, '{}'::jsonb)
)
RETURNING id, automation_id, workspace_id, published_by_type, published_by_id, config_summary, created_at"#
    )
        .bind(automation_id)
        .bind(workspace_id)
        .bind(published_by_type)
        .bind(published_by_id)
        .bind(config_summary)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRuleVersion {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        published_by_type: row.try_get(3)?,
        published_by_id: row.try_get(4)?,
        config_summary: row.try_get(5)?,
        created_at: row.try_get(6)?,
    }))
}

pub async fn create_automation_run(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    source: &str,
    status: &str,
    trigger_id: Uuid,
    trigger_payload: &serde_json::Value,
    team_id: Uuid,
    planned_at: Option<DateTime<Utc>>,
    webhook_delivery_id: Uuid,
    quota_reservation_id: Uuid,
    reason_code: Option<&str>,
    id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"INSERT INTO automation_run (
    automation_id, trigger_id, source, status, trigger_payload, team_id, planned_at,
    webhook_delivery_id, quota_reservation_id, reason_code, id
) VALUES (
    $1, $4, $2, $3, $5,
    $6, $7,
    $8, $9,
    $10, COALESCE($11::uuid, gen_random_uuid())
) RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(automation_id)
        .bind(source)
        .bind(status)
        .bind(trigger_id)
        .bind(trigger_payload)
        .bind(team_id)
        .bind(planned_at)
        .bind(webhook_delivery_id)
        .bind(quota_reservation_id)
        .bind(reason_code)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn create_automation_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    runtime_id: Uuid,
    priority: i32,
    automation_run_id: Uuid,
    trigger_summary: Option<&str>,
    originator_user_id: Uuid,
    accountable_user_id: Uuid,
    rule_version_id: Uuid,
    originator_source: Option<&str>,
    trigger_evidence_kind: Option<&str>,
    trigger_evidence_ref_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_task_queue (
    agent_id, runtime_id, issue_id, status, priority, automation_run_id, trigger_summary,
    originator_user_id, accountable_user_id, rule_version_id,
    originator_source, trigger_evidence_kind, trigger_evidence_ref_id,
    id
)
SELECT
    $1, $2, NULL, 'queued', $3, $4, $5,
    $6,
    $7,
    $8,
    $9,
    $10,
    $11,
    COALESCE($12::uuid, gen_random_uuid())
WHERE lock_task_owner_rows($1, NULL, $2)
RETURNING id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, automation_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key"#
    )
        .bind(agent_id)
        .bind(runtime_id)
        .bind(priority)
        .bind(automation_run_id)
        .bind(trigger_summary)
        .bind(originator_user_id)
        .bind(accountable_user_id)
        .bind(rule_version_id)
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
    }))
}

pub async fn create_automation_trigger(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    kind: &str,
    enabled: bool,
    cron_expression: Option<&str>,
    timezone: Option<&str>,
    next_run_at: Option<DateTime<Utc>>,
    webhook_token: Option<&str>,
    label: Option<&str>,
    provider: Option<&str>,
    event_filters: &serde_json::Value,
    published_by_type: Option<&str>,
    published_by_id: Uuid,
) -> anyhow::Result<Option<AutomationTrigger>> {
    let row = sqlx::query(
        r#"INSERT INTO automation_trigger (
    automation_id, kind, enabled, cron_expression, timezone,
    next_run_at, webhook_token, label, provider, event_filters,
    published_by_type, published_by_id
) VALUES (
    $1, $2, $3, $4, $5,
    $6, $7, $8,
    COALESCE($9::text, 'generic'),
    $10,
    $11, $12
) RETURNING id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id"#
    )
        .bind(automation_id)
        .bind(kind)
        .bind(enabled)
        .bind(cron_expression)
        .bind(timezone)
        .bind(next_run_at)
        .bind(webhook_token)
        .bind(label)
        .bind(provider)
        .bind(event_filters)
        .bind(published_by_type)
        .bind(published_by_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationTrigger {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
    }))
}

pub async fn delete_automation_collaborator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    user_type: &str,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM automation_collaborator
WHERE automation_id = $1 AND user_type = $2 AND user_id = $3"#,
    )
    .bind(automation_id)
    .bind(user_type)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_automation_collaborators_for_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM automation_collaborator
WHERE automation_id = $1"#,
    )
    .bind(automation_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_automation_subscribers_for_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM automation_subscriber
WHERE automation_id = $1"#,
    )
    .bind(automation_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_automation_trigger(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM automation_trigger WHERE id = $1"#)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FailAutomationRunsByIssueRow {
    pub id: Option<Uuid>,
    pub automation_id: Option<Uuid>,
    pub trigger_id: Option<Uuid>,
    pub source: String,
    pub status: String,
    pub issue_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub triggered_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failure_reason: Option<String>,
    pub trigger_payload: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub created_at: Option<DateTime<Utc>>,
    pub team_id: Option<Uuid>,
    pub planned_at: Option<DateTime<Utc>>,
    pub webhook_delivery_id: Option<Uuid>,
    pub quota_reservation_id: Option<Uuid>,
    pub reason_code: Option<String>,
}

pub async fn fail_automation_runs_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<FailAutomationRunsByIssueRow>> {
    let rows = sqlx::query(
        r#"WITH updated_runs AS (
    UPDATE automation_run
    SET status = 'failed', completed_at = now(), failure_reason = 'linked issue was deleted'
    WHERE issue_id = $1
      AND status IN ('issue_created', 'running')
    RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code
), locked_reservations AS MATERIALIZED (
    SELECT qr.id, qr.workspace_id, qr.period_start, qr.period_end, qr.policy_revision, qr.subscription_version, qr.source, qr.idempotency_key, qr.state, qr.created_at, qr.finalized_at
    FROM automation_quota_reservation qr
    JOIN updated_runs ar ON ar.quota_reservation_id = qr.id
    WHERE qr.state = 'reserved'
    FOR UPDATE
), released_reservations AS (
    UPDATE automation_quota_reservation AS qr
    SET state = 'released', finalized_at = now()
    FROM locked_reservations AS locked
    WHERE qr.id = locked.id
      AND EXISTS (
          SELECT 1 FROM automation_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
), released_by_period AS (
    SELECT workspace_id, period_start, period_end, count(*)::bigint AS released_count
    FROM released_reservations
    GROUP BY workspace_id, period_start, period_end
), settled_periods AS (
    UPDATE automation_quota_period AS p
    SET reserved_count = reserved_count - released.released_count,
        updated_at = now()
    FROM released_by_period AS released
    WHERE p.workspace_id = released.workspace_id
      AND p.period_start = released.period_start
      AND p.period_end = released.period_end
    RETURNING p.workspace_id
)
SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM updated_runs"#
    )
        .bind(issue_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(FailAutomationRunsByIssueRow {
            id: row.try_get(0)?,
            automation_id: row.try_get(1)?,
            trigger_id: row.try_get(2)?,
            source: row.try_get(3)?,
            status: row.try_get(4)?,
            issue_id: row.try_get(5)?,
            task_id: row.try_get(6)?,
            triggered_at: row.try_get(7)?,
            completed_at: row.try_get(8)?,
            failure_reason: row.try_get(9)?,
            trigger_payload: row.try_get(10)?,
            result: row.try_get(11)?,
            created_at: row.try_get(12)?,
            team_id: row.try_get(13)?,
            planned_at: row.try_get(14)?,
            webhook_delivery_id: row.try_get(15)?,
            quota_reservation_id: row.try_get(16)?,
            reason_code: row.try_get(17)?,
        });
    }
    Ok(out)
}

pub async fn get_active_automation_rule_version(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    automation_id: Uuid,
) -> anyhow::Result<Option<AutomationRuleVersion>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, workspace_id, published_by_type, published_by_id, config_summary, created_at FROM automation_rule_version
WHERE workspace_id = $1 AND automation_id = $2
ORDER BY created_at DESC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(automation_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRuleVersion {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        published_by_type: row.try_get(3)?,
        published_by_id: row.try_get(4)?,
        config_summary: row.try_get(5)?,
        created_at: row.try_get(6)?,
    }))
}

pub async fn get_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason FROM automation
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

pub async fn get_automation_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason FROM automation
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

pub async fn get_automation_run(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM automation_run
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn get_automation_run_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM automation_run
WHERE issue_id = $1 AND status IN ('issue_created', 'running')
LIMIT 1"#
    )
        .bind(issue_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn get_automation_run_by_quota_reservation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    quota_reservation_id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM automation_run
WHERE quota_reservation_id = $1
LIMIT 1"#
    )
        .bind(quota_reservation_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn get_automation_run_by_trigger_and_planned(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    trigger_id: Uuid,
    planned_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM automation_run
WHERE trigger_id = $1
  AND planned_at = $2
LIMIT 1"#
    )
        .bind(trigger_id)
        .bind(planned_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn get_automation_run_by_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    webhook_delivery_id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM automation_run
WHERE webhook_delivery_id = $1
LIMIT 1"#
    )
        .bind(webhook_delivery_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn get_automation_task_by_run(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_run_id: Uuid,
) -> anyhow::Result<Option<AgentTaskQueue>> {
    let row = sqlx::query(
        r#"SELECT id, agent_id, issue_id, status, priority, dispatched_at, started_at, completed_at, result, error, created_at, context, runtime_id, session_id, work_dir, trigger_comment_id, chat_session_id, automation_run_id, attempt, max_attempts, parent_task_id, failure_reason, trigger_summary, force_fresh_session, is_leader_task, wait_reason, initiator_user_id, handoff_note, prepare_lease_expires_at, team_id, runtime_mcp_overlay, escalation_for_task_id, fire_at, originator_user_id, runtime_connected_apps, coalesced_comment_ids, delivered_comment_ids, chat_input_task_id, chat_finalize_deferred_at, originator_source, delegated_from_task_id, retry_of_task_id, rerun_of_task_id, rule_version_id, trigger_evidence_kind, trigger_evidence_ref_id, accountable_user_id, session_rollout_missing, retired_session_id, quick_actions_disabled, regenerate_quick_actions_for, branch_name, durable_work_dir, execution_lane_key FROM agent_task_queue
WHERE automation_run_id = $1
ORDER BY created_at
LIMIT 1"#
    )
        .bind(automation_run_id)
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
    }))
}

pub async fn get_automation_trigger(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<AutomationTrigger>> {
    let row = sqlx::query(
        r#"SELECT id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id FROM automation_trigger
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationTrigger {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetWebhookTriggerByTokenRow {
    pub id: Option<Uuid>,
    pub automation_id: Option<Uuid>,
    pub kind: String,
    pub enabled: bool,
    pub cron_expression: Option<String>,
    pub timezone: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub webhook_token: Option<String>,
    pub label: Option<String>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub provider: String,
    pub signing_secret: Option<String>,
    pub event_filters: Option<serde_json::Value>,
    pub published_by_type: Option<String>,
    pub published_by_id: Option<Uuid>,
    pub automation_workspace_id: Option<Uuid>,
}

pub async fn get_webhook_trigger_by_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    webhook_token: Option<&str>,
) -> anyhow::Result<Option<GetWebhookTriggerByTokenRow>> {
    let row = sqlx::query(
        r#"SELECT t.id, t.automation_id, t.kind, t.enabled, t.cron_expression, t.timezone, t.next_run_at, t.webhook_token, t.label, t.last_fired_at, t.created_at, t.updated_at, t.provider, t.signing_secret, t.event_filters, t.published_by_type, t.published_by_id, a.workspace_id AS automation_workspace_id
FROM automation_trigger t
JOIN automation a ON a.id = t.automation_id
WHERE t.kind = 'webhook'
  AND t.webhook_token = $1"#
    )
        .bind(webhook_token)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetWebhookTriggerByTokenRow {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
        automation_workspace_id: row.try_get(17)?,
    }))
}

pub async fn is_automation_collaborator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
    SELECT 1 FROM automation_collaborator
    WHERE automation_id = $1 AND user_type = 'member' AND user_id = $2
) AS is_collaborator"#,
    )
    .bind(automation_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_automation_collaborators(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
) -> anyhow::Result<Vec<AutomationCollaborator>> {
    let rows = sqlx::query(
        r#"SELECT automation_id, user_type, user_id, granted_by, created_at FROM automation_collaborator
WHERE automation_id = $1
ORDER BY created_at ASC, user_id ASC"#
    )
        .bind(automation_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AutomationCollaborator {
            automation_id: row.try_get(0)?,
            user_type: row.try_get(1)?,
            user_id: row.try_get(2)?,
            granted_by: row.try_get(3)?,
            created_at: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn list_automation_i_ds_for_collaborator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT automation_id FROM automation_collaborator
WHERE user_type = 'member' AND user_id = $1"#,
    )
    .bind(user_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_automation_runs(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    limit: i32,
    offset: i32,
) -> anyhow::Result<Vec<AutomationRun>> {
    let rows = sqlx::query(
        r#"SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM automation_run
WHERE automation_id = $1
ORDER BY created_at DESC
LIMIT $2 OFFSET $3"#
    )
        .bind(automation_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AutomationRun {
            id: row.try_get(0)?,
            automation_id: row.try_get(1)?,
            trigger_id: row.try_get(2)?,
            source: row.try_get(3)?,
            status: row.try_get(4)?,
            issue_id: row.try_get(5)?,
            task_id: row.try_get(6)?,
            triggered_at: row.try_get(7)?,
            completed_at: row.try_get(8)?,
            failure_reason: row.try_get(9)?,
            trigger_payload: row.try_get(10)?,
            result: row.try_get(11)?,
            created_at: row.try_get(12)?,
            team_id: row.try_get(13)?,
            planned_at: row.try_get(14)?,
            webhook_delivery_id: row.try_get(15)?,
            quota_reservation_id: row.try_get(16)?,
            reason_code: row.try_get(17)?,
        });
    }
    Ok(out)
}

pub async fn list_automation_subscribers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
) -> anyhow::Result<Vec<AutomationSubscriber>> {
    let rows = sqlx::query(
        r#"SELECT automation_id, user_type, user_id, created_at FROM automation_subscriber
WHERE automation_id = $1
ORDER BY created_at ASC, user_id ASC"#,
    )
    .bind(automation_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AutomationSubscriber {
            automation_id: row.try_get(0)?,
            user_type: row.try_get(1)?,
            user_id: row.try_get(2)?,
            created_at: row.try_get(3)?,
        });
    }
    Ok(out)
}

pub async fn list_automation_triggers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
) -> anyhow::Result<Vec<AutomationTrigger>> {
    let rows = sqlx::query(
        r#"SELECT id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id FROM automation_trigger
WHERE automation_id = $1
ORDER BY created_at ASC"#
    )
        .bind(automation_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AutomationTrigger {
            id: row.try_get(0)?,
            automation_id: row.try_get(1)?,
            kind: row.try_get(2)?,
            enabled: row.try_get(3)?,
            cron_expression: row.try_get(4)?,
            timezone: row.try_get(5)?,
            next_run_at: row.try_get(6)?,
            webhook_token: row.try_get(7)?,
            label: row.try_get(8)?,
            last_fired_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            provider: row.try_get(12)?,
            signing_secret: row.try_get(13)?,
            event_filters: row.try_get(14)?,
            published_by_type: row.try_get(15)?,
            published_by_id: row.try_get(16)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAutomationsRow {
    pub automation: Automation,
    pub trigger_kinds: Option<Vec<String>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_status: String,
}

pub async fn list_automations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    status: Option<&str>,
) -> anyhow::Result<Vec<ListAutomationsRow>> {
    let rows = sqlx::query(
        r#"SELECT
  a.id, a.workspace_id, a.title, a.description, a.assignee_id, a.status, a.execution_mode, a.issue_title_template, a.created_by_type, a.created_by_id, a.last_run_at, a.created_at, a.updated_at, a.assignee_type, a.project_id, a.pause_reason,
  (
    SELECT array_agg(DISTINCT t.kind ORDER BY t.kind)
    FROM automation_trigger t
    WHERE t.automation_id = a.id AND t.enabled
  )::text[] AS trigger_kinds,
  (
    SELECT min(t.next_run_at)
    FROM automation_trigger t
    WHERE t.automation_id = a.id AND t.enabled AND t.kind = 'schedule'
  )::timestamptz AS next_run_at,
  COALESCE((
    SELECT r.status
    FROM automation_run r
    WHERE r.automation_id = a.id
    ORDER BY r.triggered_at DESC
    LIMIT 1
  ), '')::text AS last_run_status
FROM automation a
WHERE a.workspace_id = $1
  AND (
    ($2::text IS NULL AND a.status <> 'archived')
    OR a.status = $2
  )
ORDER BY a.created_at DESC"#
    )
        .bind(workspace_id)
        .bind(status)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAutomationsRow {
            automation: Automation {
                id: row.try_get(0)?,
                workspace_id: row.try_get(1)?,
                title: row.try_get(2)?,
                description: row.try_get(3)?,
                assignee_id: row.try_get(4)?,
                status: row.try_get(5)?,
                execution_mode: row.try_get(6)?,
                issue_title_template: row.try_get(7)?,
                created_by_type: row.try_get(8)?,
                created_by_id: row.try_get(9)?,
                last_run_at: row.try_get(10)?,
                created_at: row.try_get(11)?,
                updated_at: row.try_get(12)?,
                assignee_type: row.try_get(13)?,
                project_id: row.try_get(14)?,
                pause_reason: row.try_get(15)?,
            },
            trigger_kinds: row.try_get(16)?,
            next_run_at: row.try_get(17)?,
            last_run_status: row.try_get(18)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListSchedulableAutomationTriggersRow {
    pub id: Option<Uuid>,
    pub automation_id: Option<Uuid>,
    pub cron_expression: Option<String>,
    pub timezone: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
}

pub async fn list_schedulable_automation_triggers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<Vec<ListSchedulableAutomationTriggersRow>> {
    let rows = sqlx::query(
        r#"SELECT t.id, t.automation_id, t.cron_expression, t.timezone, t.created_at, t.last_fired_at
FROM automation_trigger t
JOIN automation a ON a.id = t.automation_id
WHERE t.kind = 'schedule'
  AND t.enabled = TRUE
  AND a.status = 'active'
  AND t.cron_expression IS NOT NULL
  AND t.cron_expression <> ''
ORDER BY t.id"#,
    )
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListSchedulableAutomationTriggersRow {
            id: row.try_get(0)?,
            automation_id: row.try_get(1)?,
            cron_expression: row.try_get(2)?,
            timezone: row.try_get(3)?,
            created_at: row.try_get(4)?,
            last_fired_at: row.try_get(5)?,
        });
    }
    Ok(out)
}

pub async fn lock_automation_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason FROM automation
WHERE id = $1 AND workspace_id = $2
FOR UPDATE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

pub async fn pause_automations_by_unbound_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Automation>> {
    let rows = sqlx::query(
        r#"UPDATE automation a
SET status = 'paused',
    pause_reason = 'agent_runtime_required',
    updated_at = now()
WHERE a.status = 'active'
  AND (
    (a.assignee_type = 'agent' AND a.assignee_id = ANY($1::uuid[]))
    OR (
      a.assignee_type = 'team'
      AND EXISTS (
        SELECT 1
        FROM team s
        WHERE s.id = a.assignee_id
          AND s.leader_id = ANY($1::uuid[])
      )
    )
  )
RETURNING a.id, a.workspace_id, a.title, a.description, a.assignee_id, a.status, a.execution_mode, a.issue_title_template, a.created_by_type, a.created_by_id, a.last_run_at, a.created_at, a.updated_at, a.assignee_type, a.project_id, a.pause_reason"#
    )
        .bind(agent_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Automation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            assignee_id: row.try_get(4)?,
            status: row.try_get(5)?,
            execution_mode: row.try_get(6)?,
            issue_title_template: row.try_get(7)?,
            created_by_type: row.try_get(8)?,
            created_by_id: row.try_get(9)?,
            last_run_at: row.try_get(10)?,
            created_at: row.try_get(11)?,
            updated_at: row.try_get(12)?,
            assignee_type: row.try_get(13)?,
            project_id: row.try_get(14)?,
            pause_reason: row.try_get(15)?,
        });
    }
    Ok(out)
}

pub async fn pause_automations_by_unrunnable_team(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    team_id: Uuid,
) -> anyhow::Result<Vec<Automation>> {
    let rows = sqlx::query(
        r#"UPDATE automation
SET status = 'paused',
    pause_reason = 'agent_runtime_required',
    updated_at = now()
WHERE status = 'active'
  AND assignee_type = 'team'
  AND assignee_id = $1
RETURNING id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason"#
    )
        .bind(team_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Automation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            assignee_id: row.try_get(4)?,
            status: row.try_get(5)?,
            execution_mode: row.try_get(6)?,
            issue_title_template: row.try_get(7)?,
            created_by_type: row.try_get(8)?,
            created_by_id: row.try_get(9)?,
            last_run_at: row.try_get(10)?,
            created_at: row.try_get(11)?,
            updated_at: row.try_get(12)?,
            assignee_type: row.try_get(13)?,
            project_id: row.try_get(14)?,
            pause_reason: row.try_get(15)?,
        });
    }
    Ok(out)
}

pub async fn recover_partial_automation_run(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"WITH updated_run AS (
    UPDATE automation_run AS ar
    SET status = 'failed',
        completed_at = now(),
        failure_reason = 'recovered partial dispatch (crashed before downstream creation)',
        reason_code = 'internal_error',
        planned_at = NULL
    WHERE ar.id = $1
      AND (
          ar.status = 'pending'
          OR (ar.status = 'issue_created' AND ar.issue_id IS NULL)
          OR (ar.status = 'running' AND ar.task_id IS NULL)
      )
      AND NOT EXISTS (
          SELECT 1
          FROM agent_task_queue task
          WHERE task.automation_run_id = ar.id
      )
    RETURNING ar.quota_reservation_id
), locked_reservation AS MATERIALIZED (
    SELECT qr.id, qr.workspace_id, qr.period_start, qr.period_end, qr.policy_revision, qr.subscription_version, qr.source, qr.idempotency_key, qr.state, qr.created_at, qr.finalized_at
    FROM automation_quota_reservation qr
    JOIN updated_run ar ON ar.quota_reservation_id = qr.id
    WHERE qr.state = 'reserved'
    FOR UPDATE
), released_reservation AS (
    UPDATE automation_quota_reservation AS qr
    SET state = 'released', finalized_at = now()
    FROM locked_reservation AS locked
    WHERE qr.id = locked.id
      AND EXISTS (
          SELECT 1 FROM automation_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
), settled_period AS (
    UPDATE automation_quota_period AS p
    SET reserved_count = reserved_count - 1,
        updated_at = now()
    FROM released_reservation AS released
    WHERE p.workspace_id = released.workspace_id
      AND p.period_start = released.period_start
      AND p.period_end = released.period_end
    RETURNING p.workspace_id
)
SELECT count(*)::bigint FROM updated_run"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn rotate_automation_trigger_webhook_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    webhook_token: Option<&str>,
) -> anyhow::Result<Option<AutomationTrigger>> {
    let row = sqlx::query(
        r#"UPDATE automation_trigger
SET webhook_token = $2,
    updated_at = now()
WHERE id = $1
  AND kind = 'webhook'
RETURNING id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id"#
    )
        .bind(id)
        .bind(webhook_token)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationTrigger {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SelectAutomationsExceedingFailureThresholdRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub title: String,
    pub assignee_id: Option<Uuid>,
    pub created_by_type: String,
    pub created_by_id: Option<Uuid>,
    pub total_runs: i64,
    pub failed_runs: i64,
}

pub async fn select_automations_exceeding_failure_threshold(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    min_runs: i64,
    fail_ratio_threshold: f64,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<SelectAutomationsExceedingFailureThresholdRow>> {
    let rows = sqlx::query(
        r#"WITH stats AS (
    SELECT automation_id,
           count(*) FILTER (WHERE status IN ('completed', 'failed')) AS total,
           count(*) FILTER (WHERE status = 'failed') AS failed
    FROM automation_run
    WHERE created_at >= $3::timestamptz
    GROUP BY automation_id
)
SELECT a.id, a.workspace_id, a.title, a.assignee_id,
       a.created_by_type, a.created_by_id,
       s.total::bigint  AS total_runs,
       s.failed::bigint AS failed_runs
FROM automation a
JOIN stats s ON s.automation_id = a.id
WHERE a.status = 'active'
  AND s.total >= $1::bigint
  AND s.failed::float8 / NULLIF(s.total, 0)::float8 >= $2::float8
ORDER BY s.failed DESC, a.id ASC"#,
    )
    .bind(min_runs)
    .bind(fail_ratio_threshold)
    .bind(since)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(SelectAutomationsExceedingFailureThresholdRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            assignee_id: row.try_get(3)?,
            created_by_type: row.try_get(4)?,
            created_by_id: row.try_get(5)?,
            total_runs: row.try_get(6)?,
            failed_runs: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn set_automation_trigger_publisher(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    published_by_type: Option<&str>,
    published_by_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE automation_trigger
SET published_by_type = $2, published_by_id = $3, updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(published_by_type)
    .bind(published_by_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_automation_trigger_publishers_by_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    automation_id: Uuid,
    published_by_type: Option<&str>,
    published_by_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE automation_trigger
SET published_by_type = $2, published_by_id = $3, updated_at = now()
WHERE automation_id = $1"#,
    )
    .bind(automation_id)
    .bind(published_by_type)
    .bind(published_by_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_automation_trigger_signing_secret(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    signing_secret: Option<&str>,
) -> anyhow::Result<Option<AutomationTrigger>> {
    let row = sqlx::query(
        r#"UPDATE automation_trigger
SET signing_secret = $2,
    updated_at = now()
WHERE id = $1
  AND kind = 'webhook'
RETURNING id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id"#
    )
        .bind(id)
        .bind(signing_secret)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationTrigger {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
    }))
}

pub async fn set_automation_trigger_webhook_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    webhook_token: Option<&str>,
) -> anyhow::Result<Option<AutomationTrigger>> {
    let row = sqlx::query(
        r#"UPDATE automation_trigger
SET webhook_token = $2,
    updated_at = now()
WHERE id = $1
RETURNING id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id"#
    )
        .bind(id)
        .bind(webhook_token)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationTrigger {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
    }))
}

pub async fn system_pause_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"UPDATE automation
SET status = 'paused', pause_reason = NULL, updated_at = now()
WHERE id = $1 AND status = 'active'
RETURNING id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

/// Workspace-scoped variant used by cross-tenant background sweeps. The
/// status predicate makes retries and concurrent monitors idempotent: only
/// the caller that transitions the row receives it back.
pub async fn system_pause_automation_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"UPDATE automation
SET status = 'paused', pause_reason = NULL, updated_at = now()
WHERE id = $1 AND workspace_id = $2 AND status = 'active'
RETURNING id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

pub async fn touch_automation_trigger_fired_at(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE automation_trigger
SET last_fired_at = now(),
    updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_automation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
    assignee_type: Option<&str>,
    assignee_id: Uuid,
    status: Option<&str>,
    execution_mode: Option<&str>,
    issue_title_template: Option<&str>,
    project_id: Uuid,
) -> anyhow::Result<Option<Automation>> {
    let row = sqlx::query(
        r#"UPDATE automation SET
    title = COALESCE($2, title),
    description = COALESCE($3, description),
    assignee_type = COALESCE($4, assignee_type),
    assignee_id = COALESCE($5::uuid, assignee_id),
    status = COALESCE($6, status),
    pause_reason = CASE
      WHEN $6::text IS NOT NULL THEN NULL
      ELSE pause_reason
    END,
    execution_mode = COALESCE($7, execution_mode),
    issue_title_template = $8,
    project_id = $9,
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, title, description, assignee_id, status, execution_mode, issue_title_template, created_by_type, created_by_id, last_run_at, created_at, updated_at, assignee_type, project_id, pause_reason"#
    )
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(assignee_type)
        .bind(assignee_id)
        .bind(status)
        .bind(execution_mode)
        .bind(issue_title_template)
        .bind(project_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Automation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_id: row.try_get(4)?,
        status: row.try_get(5)?,
        execution_mode: row.try_get(6)?,
        issue_title_template: row.try_get(7)?,
        created_by_type: row.try_get(8)?,
        created_by_id: row.try_get(9)?,
        last_run_at: row.try_get(10)?,
        created_at: row.try_get(11)?,
        updated_at: row.try_get(12)?,
        assignee_type: row.try_get(13)?,
        project_id: row.try_get(14)?,
        pause_reason: row.try_get(15)?,
    }))
}

pub async fn update_automation_last_run_at(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE automation SET last_run_at = now(), updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_automation_run_completed(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    result: &serde_json::Value,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"UPDATE automation_run
SET status = 'completed', completed_at = now(), result = $2
WHERE id = $1
RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(id)
        .bind(result)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn update_automation_run_failed(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    failure_reason: Option<&str>,
    reason_code: Option<&str>,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"UPDATE automation_run
SET status = 'failed', completed_at = now(), failure_reason = $2,
    reason_code = $3
WHERE id = $1
RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(id)
        .bind(failure_reason)
        .bind(reason_code)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn update_automation_run_issue_created(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"UPDATE automation_run
SET status = 'issue_created', issue_id = $2
WHERE id = $1
RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(id)
        .bind(issue_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn update_automation_run_running(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"UPDATE automation_run
SET status = 'running', task_id = $2
WHERE id = $1
RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(id)
        .bind(task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn update_automation_run_skipped(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    failure_reason: Option<&str>,
    reason_code: Option<&str>,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"UPDATE automation_run
SET status = 'skipped', completed_at = now(), failure_reason = $2,
    reason_code = $3
WHERE id = $1
RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(id)
        .bind(failure_reason)
        .bind(reason_code)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn update_automation_run_skipped_with_result(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    failure_reason: Option<&str>,
    result: &serde_json::Value,
) -> anyhow::Result<Option<AutomationRun>> {
    let row = sqlx::query(
        r#"UPDATE automation_run
SET status = 'skipped',
    completed_at = now(),
    failure_reason = $2,
    result = $3
WHERE id = $1
RETURNING id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code"#
    )
        .bind(id)
        .bind(failure_reason)
        .bind(result)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationRun {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateAutomationRunTerminalWithQuotaRow {
    pub id: Option<Uuid>,
    pub automation_id: Option<Uuid>,
    pub trigger_id: Option<Uuid>,
    pub source: String,
    pub status: String,
    pub issue_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub triggered_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failure_reason: Option<String>,
    pub trigger_payload: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub created_at: Option<DateTime<Utc>>,
    pub team_id: Option<Uuid>,
    pub planned_at: Option<DateTime<Utc>>,
    pub webhook_delivery_id: Option<Uuid>,
    pub quota_reservation_id: Option<Uuid>,
    pub reason_code: Option<String>,
}

pub async fn update_automation_run_terminal_with_quota(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    terminal_status: &str,
    result: &serde_json::Value,
    failure_reason: Option<&str>,
    reason_code: Option<&str>,
    run_id: Uuid,
    consume: bool,
) -> anyhow::Result<Option<UpdateAutomationRunTerminalWithQuotaRow>> {
    let row = sqlx::query(
        r#"WITH updated_run AS (
    UPDATE automation_run AS ar
    SET status = $1::text,
        completed_at = now(),
        result = CASE
            WHEN $1::text = 'completed' THEN $2::jsonb
            ELSE ar.result
        END,
        failure_reason = CASE
            WHEN $1::text IN ('failed', 'skipped') THEN $3::text
            ELSE ar.failure_reason
        END,
        reason_code = CASE
            WHEN $1::text IN ('failed', 'skipped') THEN $4::text
            ELSE ar.reason_code
        END
    WHERE ar.id = $5
    RETURNING ar.id, ar.automation_id, ar.trigger_id, ar.source, ar.status, ar.issue_id, ar.task_id, ar.triggered_at, ar.completed_at, ar.failure_reason, ar.trigger_payload, ar.result, ar.created_at, ar.team_id, ar.planned_at, ar.webhook_delivery_id, ar.quota_reservation_id, ar.reason_code
), locked_reservation AS MATERIALIZED (
    SELECT qr.id, qr.workspace_id, qr.period_start, qr.period_end, qr.policy_revision, qr.subscription_version, qr.source, qr.idempotency_key, qr.state, qr.created_at, qr.finalized_at
    FROM automation_quota_reservation qr
    JOIN updated_run ar ON ar.quota_reservation_id = qr.id
    WHERE qr.state = 'reserved'
    FOR UPDATE
), finalized_reservation AS (
    UPDATE automation_quota_reservation AS qr
    SET state = CASE WHEN $6::boolean THEN 'consumed' ELSE 'released' END,
        finalized_at = now()
    FROM locked_reservation AS locked
    WHERE qr.id = locked.id
      AND EXISTS (
          SELECT 1 FROM automation_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
), settled_period AS (
    UPDATE automation_quota_period AS p
    SET reserved_count = reserved_count - 1,
        used_count = used_count + CASE WHEN $6::boolean THEN 1 ELSE 0 END,
        updated_at = now()
    FROM finalized_reservation AS finalized
    WHERE p.workspace_id = finalized.workspace_id
      AND p.period_start = finalized.period_start
      AND p.period_end = finalized.period_end
    RETURNING p.workspace_id
)
SELECT id, automation_id, trigger_id, source, status, issue_id, task_id, triggered_at, completed_at, failure_reason, trigger_payload, result, created_at, team_id, planned_at, webhook_delivery_id, quota_reservation_id, reason_code FROM updated_run"#
    )
        .bind(terminal_status)
        .bind(result)
        .bind(failure_reason)
        .bind(reason_code)
        .bind(run_id)
        .bind(consume)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(UpdateAutomationRunTerminalWithQuotaRow {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        trigger_id: row.try_get(2)?,
        source: row.try_get(3)?,
        status: row.try_get(4)?,
        issue_id: row.try_get(5)?,
        task_id: row.try_get(6)?,
        triggered_at: row.try_get(7)?,
        completed_at: row.try_get(8)?,
        failure_reason: row.try_get(9)?,
        trigger_payload: row.try_get(10)?,
        result: row.try_get(11)?,
        created_at: row.try_get(12)?,
        team_id: row.try_get(13)?,
        planned_at: row.try_get(14)?,
        webhook_delivery_id: row.try_get(15)?,
        quota_reservation_id: row.try_get(16)?,
        reason_code: row.try_get(17)?,
    }))
}

pub async fn update_automation_trigger(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    enabled: Option<bool>,
    cron_expression: Option<&str>,
    timezone: Option<&str>,
    next_run_at: Option<DateTime<Utc>>,
    label: Option<&str>,
    event_filters: &serde_json::Value,
) -> anyhow::Result<Option<AutomationTrigger>> {
    let row = sqlx::query(
        r#"UPDATE automation_trigger SET
    enabled = COALESCE($2::boolean, enabled),
    cron_expression = COALESCE($3, cron_expression),
    timezone = COALESCE($4, timezone),
    next_run_at = $5,
    label = COALESCE($6, label),
    event_filters = COALESCE($7, event_filters),
    updated_at = now()
WHERE id = $1
RETURNING id, automation_id, kind, enabled, cron_expression, timezone, next_run_at, webhook_token, label, last_fired_at, created_at, updated_at, provider, signing_secret, event_filters, published_by_type, published_by_id"#
    )
        .bind(id)
        .bind(enabled)
        .bind(cron_expression)
        .bind(timezone)
        .bind(next_run_at)
        .bind(label)
        .bind(event_filters)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutomationTrigger {
        id: row.try_get(0)?,
        automation_id: row.try_get(1)?,
        kind: row.try_get(2)?,
        enabled: row.try_get(3)?,
        cron_expression: row.try_get(4)?,
        timezone: row.try_get(5)?,
        next_run_at: row.try_get(6)?,
        webhook_token: row.try_get(7)?,
        label: row.try_get(8)?,
        last_fired_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        provider: row.try_get(12)?,
        signing_secret: row.try_get(13)?,
        event_filters: row.try_get(14)?,
        published_by_type: row.try_get(15)?,
        published_by_id: row.try_get(16)?,
    }))
}
