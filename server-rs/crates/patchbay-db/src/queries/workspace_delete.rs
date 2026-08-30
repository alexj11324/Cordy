//! Typed SQL queries for workspace_delete records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn delete_task_batch(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_ids: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
batch AS MATERIALIZED (
    SELECT id FROM unnest($1::uuid[]) AS t(id)
),
deleted_task_usage AS (
    DELETE FROM task_usage WHERE task_id IN (SELECT id FROM batch)
),
deleted_task_messages AS (
    DELETE FROM task_message WHERE task_id IN (SELECT id FROM batch)
),
deleted_task_tokens AS (
    DELETE FROM task_token WHERE task_id IN (SELECT id FROM batch)
),
deleted_channel_outbound_cards AS (
    DELETE FROM channel_outbound_card_message WHERE task_id IN (SELECT id FROM batch)
),
deleted_lark_outbound_cards AS (
    DELETE FROM lark_outbound_card_message WHERE task_id IN (SELECT id FROM batch)
),
deleted_draft_restores AS (
    DELETE FROM chat_draft_restore WHERE task_id IN (SELECT id FROM batch)
)
DELETE FROM agent_task_queue WHERE id IN (SELECT id FROM batch)"#,
    )
    .bind(task_ids)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_task_tokens_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM task_token WHERE agent_id = $1"#)
        .bind(agent_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_administration(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
deleted_members AS (
    DELETE FROM member WHERE member.workspace_id = $1
),
deleted_notification_preferences AS (
    DELETE FROM notification_preference
    WHERE notification_preference.workspace_id = $1
),
deleted_pins AS (
    DELETE FROM pinned_item WHERE pinned_item.workspace_id = $1
),
deleted_daemon_tokens AS (
    DELETE FROM daemon_token WHERE daemon_token.workspace_id = $1
),
detached_feedback AS (
    UPDATE feedback
    SET workspace_id = NULL
    WHERE feedback.workspace_id = $1
),
detached_client_usage AS (
    UPDATE client_usage_daily
    SET workspace_id = NULL
    WHERE client_usage_daily.workspace_id = $1
),
deleted_share_links AS (
    DELETE FROM workspace_share_link
    WHERE workspace_share_link.workspace_id = $1
)
DELETE FROM workspace_invitation
WHERE workspace_invitation.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent WHERE agent.workspace_id = $1"#)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_autopilot_children(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
deleted_triggers AS (
    DELETE FROM autopilot_trigger
    WHERE autopilot_id IN (
        SELECT id FROM autopilot WHERE autopilot.workspace_id = $1
    )
)
DELETE FROM autopilot_rule_version
WHERE autopilot_rule_version.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_autopilot_quota_periods(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM autopilot_quota_period
WHERE autopilot_quota_period.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_autopilot_quota_reservations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM autopilot_quota_reservation
WHERE autopilot_quota_reservation.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_autopilot_runs(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM autopilot_run
WHERE autopilot_id IN (
    SELECT id FROM autopilot WHERE autopilot.workspace_id = $1
)"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_autopilots(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM autopilot WHERE autopilot.workspace_id = $1"#)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_chat_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM chat_message
WHERE chat_session_id IN (
    SELECT id FROM chat_session WHERE chat_session.workspace_id = $1
)"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_comments(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM comment WHERE comment.workspace_id = $1"#)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_communication_roots(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
deleted_sessions AS (
    DELETE FROM chat_session WHERE chat_session.workspace_id = $1
),
deleted_channel_installations AS (
    DELETE FROM channel_installation
    WHERE channel_installation.workspace_id = $1
)
DELETE FROM lark_installation WHERE lark_installation.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_connections(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH deleted_github_installations AS (
    DELETE FROM github_installation
    WHERE github_installation.workspace_id = $1
)
DELETE FROM vcs_connection WHERE vcs_connection.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_issue_roots(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
deleted_coordination_assignments AS (
    DELETE FROM agent_coordination_assignment WHERE workspace_id = $1
),
deleted_coordination_outbox AS (
    DELETE FROM agent_coordination_outbox WHERE workspace_id = $1
),
deleted_issues AS (
    DELETE FROM issue WHERE issue.workspace_id = $1
),
deleted_labels AS (
    DELETE FROM issue_label WHERE issue_label.workspace_id = $1
),
deleted_properties AS (
    DELETE FROM issue_property WHERE issue_property.workspace_id = $1
),
deleted_issue_views AS (
    DELETE FROM issue_view WHERE issue_view.workspace_id = $1
),
deleted_issue_view_preferences AS (
    DELETE FROM issue_view_preference
    WHERE issue_view_preference.workspace_id = $1
)
DELETE FROM quick_action WHERE quick_action.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_leaf_data(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
ws_agents AS MATERIALIZED (
    SELECT id FROM agent WHERE workspace_id = $1
),
ws_issues AS MATERIALIZED (
    SELECT id FROM issue WHERE workspace_id = $1
),
ws_labels AS MATERIALIZED (
    SELECT id FROM issue_label WHERE workspace_id = $1
),
ws_skills AS MATERIALIZED (
    SELECT id FROM skill WHERE workspace_id = $1
),
ws_teams AS MATERIALIZED (
    SELECT id FROM team WHERE workspace_id = $1
),
ws_sessions AS MATERIALIZED (
    SELECT id FROM chat_session WHERE workspace_id = $1
),
ws_autopilots AS MATERIALIZED (
    SELECT id FROM autopilot WHERE workspace_id = $1
),
ws_github_prs AS MATERIALIZED (
    SELECT id FROM github_pull_request WHERE workspace_id = $1
),
ws_vcs_prs AS MATERIALIZED (
    SELECT id FROM vcs_pull_request WHERE workspace_id = $1
),
ws_vcs_connections AS MATERIALIZED (
    SELECT id FROM vcs_connection WHERE workspace_id = $1
),
ws_channel_installations AS MATERIALIZED (
    SELECT id FROM channel_installation WHERE workspace_id = $1
),
ws_lark_installations AS MATERIALIZED (
    SELECT id FROM lark_installation WHERE workspace_id = $1
),
deleted_task_tokens AS (
    DELETE FROM task_token
    WHERE workspace_id = $1
),
deleted_authorization_audit AS (
    DELETE FROM authorization_audit_event WHERE workspace_id = $1
),
deleted_authorization_grants AS (
    DELETE FROM authorization_grant WHERE workspace_id = $1
),
deleted_hourly_dirty AS (
    DELETE FROM task_usage_hourly_dirty WHERE workspace_id = $1
),
deleted_hourly AS (
    DELETE FROM task_usage_hourly WHERE workspace_id = $1
),
deleted_attachments AS (
    DELETE FROM attachment WHERE workspace_id = $1
),
deleted_channel_outbound_cards AS (
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT id FROM ws_sessions)
),
deleted_lark_outbound_cards AS (
    DELETE FROM lark_outbound_card_message
    WHERE chat_session_id IN (SELECT id FROM ws_sessions)
),
deleted_draft_restores AS (
    DELETE FROM chat_draft_restore
    WHERE chat_session_id IN (SELECT id FROM ws_sessions)
),
deleted_agent_builder_drafts AS (
    DELETE FROM agent_builder_draft WHERE workspace_id = $1
),
deleted_comment_reactions AS (
    DELETE FROM comment_reaction WHERE workspace_id = $1
),
deleted_issue_reactions AS (
    DELETE FROM issue_reaction WHERE workspace_id = $1
),
deleted_activity AS (
    DELETE FROM activity_log WHERE workspace_id = $1
),
deleted_inbox AS (
    DELETE FROM inbox_item WHERE workspace_id = $1
),
deleted_workspace_channel_messages AS (
    DELETE FROM workspace_channel_message WHERE workspace_id = $1
),
deleted_workspace_channels AS (
    DELETE FROM workspace_channel WHERE workspace_id = $1
),
deleted_dependency_graph_edges AS (
    DELETE FROM dependency_graph_edge WHERE workspace_id = $1
),
deleted_dependency_graph_nodes AS (
    DELETE FROM dependency_graph_node WHERE workspace_id = $1
),
deleted_dependency_graph_plans AS (
    DELETE FROM dependency_graph_plan WHERE workspace_id = $1
),
deleted_issue_dependencies AS (
    DELETE FROM issue_dependency
    WHERE issue_id IN (SELECT id FROM ws_issues)
       OR depends_on_issue_id IN (SELECT id FROM ws_issues)
),
deleted_issue_subscribers AS (
    DELETE FROM issue_subscriber
    WHERE issue_id IN (SELECT id FROM ws_issues)
),
deleted_issue_labels AS (
    DELETE FROM issue_to_label
    WHERE issue_id IN (SELECT id FROM ws_issues)
       OR label_id IN (SELECT id FROM ws_labels)
),
deleted_agent_labels AS (
    DELETE FROM agent_to_label
    WHERE agent_id IN (SELECT id FROM ws_agents)
       OR label_id IN (SELECT id FROM ws_labels)
),
deleted_skill_labels AS (
    DELETE FROM skill_to_label
    WHERE skill_id IN (SELECT id FROM ws_skills)
       OR label_id IN (SELECT id FROM ws_labels)
),
deleted_issue_github_links AS (
    DELETE FROM issue_pull_request
    WHERE issue_id IN (SELECT id FROM ws_issues)
       OR pull_request_id IN (SELECT id FROM ws_github_prs)
),
deleted_issue_vcs_links AS (
    DELETE FROM issue_vcs_pull_request
    WHERE issue_id IN (SELECT id FROM ws_issues)
       OR pull_request_id IN (SELECT id FROM ws_vcs_prs)
),
deleted_agent_invocation_targets AS (
    DELETE FROM agent_invocation_target
    WHERE agent_id IN (SELECT id FROM ws_agents)
),
deleted_agent_skills AS (
    DELETE FROM agent_skill
    WHERE agent_id IN (SELECT id FROM ws_agents)
       OR skill_id IN (SELECT id FROM ws_skills)
),
deleted_skill_files AS (
    DELETE FROM skill_file
    WHERE skill_id IN (SELECT id FROM ws_skills)
),
deleted_daemon_connections AS (
    DELETE FROM daemon_connection
    WHERE agent_id IN (SELECT id FROM ws_agents)
),
deleted_team_members AS (
    DELETE FROM team_member
    WHERE team_id IN (SELECT id FROM ws_teams)
),
deleted_project_resources AS (
    DELETE FROM project_resource WHERE workspace_id = $1
),
deleted_autopilot_collaborators AS (
    DELETE FROM autopilot_collaborator
    WHERE autopilot_id IN (SELECT id FROM ws_autopilots)
),
deleted_autopilot_subscribers AS (
    DELETE FROM autopilot_subscriber
    WHERE autopilot_id IN (SELECT id FROM ws_autopilots)
),
deleted_webhook_deliveries AS (
    DELETE FROM webhook_delivery WHERE workspace_id = $1
),
deleted_github_check_runs AS (
    DELETE FROM github_pull_request_check_run
    WHERE pr_id IN (SELECT id FROM ws_github_prs)
),
deleted_github_check_suites AS (
    DELETE FROM github_pull_request_check_suite
    WHERE pr_id IN (SELECT id FROM ws_github_prs)
),
deleted_pending_github_suites AS (
    DELETE FROM github_pending_check_suite WHERE workspace_id = $1
),
deleted_vcs_commit_statuses AS (
    DELETE FROM vcs_commit_status
    WHERE connection_id IN (SELECT id FROM ws_vcs_connections)
),
deleted_channel_chat_bindings AS (
    DELETE FROM channel_chat_session_binding
    WHERE installation_id IN (SELECT id FROM ws_channel_installations)
       OR chat_session_id IN (SELECT id FROM ws_sessions)
),
deleted_dingtalk_group_routes AS (
    DELETE FROM dingtalk_group_route WHERE workspace_id = $1
),
deleted_channel_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup
    WHERE installation_id IN (SELECT id FROM ws_channel_installations)
),
deleted_channel_inbound_audit AS (
    DELETE FROM channel_inbound_audit
    WHERE installation_id IN (SELECT id FROM ws_channel_installations)
),
deleted_channel_user_bindings AS (
    DELETE FROM channel_user_binding WHERE workspace_id = $1
),
deleted_channel_binding_tokens AS (
    DELETE FROM channel_binding_token WHERE workspace_id = $1
),
deleted_lark_chat_bindings AS (
    DELETE FROM lark_chat_session_binding
    WHERE installation_id IN (SELECT id FROM ws_lark_installations)
       OR chat_session_id IN (SELECT id FROM ws_sessions)
),
deleted_lark_inbound_dedup AS (
    DELETE FROM lark_inbound_message_dedup
    WHERE installation_id IN (SELECT id FROM ws_lark_installations)
),
deleted_lark_inbound_audit AS (
    DELETE FROM lark_inbound_audit
    WHERE installation_id IN (SELECT id FROM ws_lark_installations)
),
deleted_lark_user_bindings AS (
    DELETE FROM lark_user_binding WHERE workspace_id = $1
),
deleted_lark_binding_tokens AS (
    DELETE FROM lark_binding_token WHERE workspace_id = $1
)
UPDATE channel_media_pending_object
SET state = CASE
        WHEN state = 'tombstoned' THEN 'tombstoned'
        ELSE 'deleting'
    END,
    lease_token = NULL,
    lease_expires_at = NULL,
    next_attempt_at = now(),
    last_error = NULL
WHERE channel_media_pending_object.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_plugin_data(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH installations AS MATERIALIZED (
    SELECT plugin_installation.id
    FROM plugin_installation
    WHERE plugin_installation.workspace_id = $1
),
deleted_storage AS (
    DELETE FROM plugin_storage
    WHERE installation_id IN (SELECT id FROM installations)
),
deleted_secrets AS (
    DELETE FROM plugin_secret
    WHERE installation_id IN (SELECT id FROM installations)
),
deleted_invocations AS (
    DELETE FROM plugin_invocation
    WHERE workspace_id = $1
)
DELETE FROM plugin_installation WHERE id IN (SELECT id FROM installations)"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_pull_requests(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH deleted_github_prs AS (
    DELETE FROM github_pull_request
    WHERE github_pull_request.workspace_id = $1
)
DELETE FROM vcs_pull_request WHERE vcs_pull_request.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_runtimes_and_projects(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
deleted_runtimes AS (
    DELETE FROM agent_runtime WHERE agent_runtime.workspace_id = $1
),
deleted_profiles AS (
    DELETE FROM runtime_profile WHERE runtime_profile.workspace_id = $1
)
DELETE FROM project WHERE project.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_teams_and_skills(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH deleted_teams AS (
    DELETE FROM team WHERE team.workspace_id = $1
)
DELETE FROM skill WHERE skill.workspace_id = $1"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn detach_task_batch_references(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_ids: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH detached_runs AS (
    UPDATE autopilot_run
    SET task_id = NULL
    WHERE task_id = ANY($1::uuid[])
)
UPDATE agent_task_queue
SET parent_task_id = NULL
WHERE parent_task_id = ANY($1::uuid[])"#,
    )
    .bind(task_ids)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_task_i_ds_by_agent_first_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_task_queue
WHERE agent_id = $1
ORDER BY id
LIMIT $2
FOR UPDATE"#,
    )
    .bind(agent_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_task_i_ds_by_agent_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_task_queue
WHERE agent_id = $1 AND id > $2
ORDER BY id
LIMIT $3
FOR UPDATE"#,
    )
    .bind(agent_id)
    .bind(id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_task_i_ds_by_issue_first_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_task_queue
WHERE issue_id = $1
ORDER BY id
LIMIT $2
FOR UPDATE"#,
    )
    .bind(issue_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_task_i_ds_by_issue_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_task_queue
WHERE issue_id = $1 AND id > $2
ORDER BY id
LIMIT $3
FOR UPDATE"#,
    )
    .bind(issue_id)
    .bind(id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_task_i_ds_by_runtime_first_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_task_queue
WHERE runtime_id = $1
ORDER BY id
LIMIT $2
FOR UPDATE"#,
    )
    .bind(runtime_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_task_i_ds_by_runtime_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_task_queue
WHERE runtime_id = $1 AND id > $2
ORDER BY id
LIMIT $3
FOR UPDATE"#,
    )
    .bind(runtime_id)
    .bind(id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_workspace_agent_id_first_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent
WHERE agent.workspace_id = $1
ORDER BY id
LIMIT $2"#,
    )
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_workspace_agent_id_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent
WHERE agent.workspace_id = $1 AND id > $2
ORDER BY id
LIMIT $3"#,
    )
    .bind(workspace_id)
    .bind(id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_workspace_issue_id_first_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM issue
WHERE issue.workspace_id = $1
ORDER BY id
LIMIT $2"#,
    )
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_workspace_issue_id_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM issue
WHERE issue.workspace_id = $1 AND id > $2
ORDER BY id
LIMIT $3"#,
    )
    .bind(workspace_id)
    .bind(id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_workspace_runtime_id_first_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_runtime
WHERE agent_runtime.workspace_id = $1
ORDER BY id
LIMIT $2"#,
    )
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_workspace_runtime_id_page(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_runtime
WHERE agent_runtime.workspace_id = $1 AND id > $2
ORDER BY id
LIMIT $3"#,
    )
    .bind(workspace_id)
    .bind(id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn lock_task_usage_rollup_for_workspace_delete(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"SELECT pg_advisory_xact_lock(4246)"#)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn lock_workspace_task_owner_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"SELECT 1 FROM agent WHERE agent.workspace_id = $1 FOR UPDATE"#)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn lock_workspace_task_owner_issues(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"SELECT 1 FROM issue WHERE issue.workspace_id = $1 FOR UPDATE"#)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn lock_workspace_task_owner_runtimes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT 1 FROM agent_runtime WHERE agent_runtime.workspace_id = $1 FOR UPDATE"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn prepare_workspace_deletion_links(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH
detached_comments AS (
    UPDATE comment
    SET parent_id = NULL
    WHERE comment.workspace_id = $1
      AND parent_id IS NOT NULL
),
detached_issues AS (
    UPDATE issue
    SET parent_issue_id = NULL
    WHERE issue.workspace_id = $1
      AND parent_issue_id IS NOT NULL
)
UPDATE webhook_delivery
SET replayed_from_delivery_id = NULL
WHERE webhook_delivery.workspace_id = $1
  AND replayed_from_delivery_id IS NOT NULL"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_workspace_teardown_mode(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"SELECT set_config('patchbay.workspace_teardown', 'on', true)"#)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}
