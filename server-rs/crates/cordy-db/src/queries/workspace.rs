//! Port of server/pkg/db/queries/workspace.sql (generated workspace.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    name: &str,
    slug: &str,
    description: Option<&str>,
    context: Option<&str>,
    issue_prefix: &str,
) -> anyhow::Result<Option<Workspace>> {
    let row = sqlx::query(
        r#"INSERT INTO workspace (name, slug, description, context, issue_prefix)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, name, slug, description, settings, created_at, updated_at, context, repos, issue_prefix, issue_counter, avatar_url, attribution_fail_closed"#
    )
        .bind(name)
        .bind(slug)
        .bind(description)
        .bind(context)
        .bind(issue_prefix)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Workspace {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        slug: row.try_get(2)?,
        description: row.try_get(3)?,
        settings: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
        context: row.try_get(7)?,
        repos: row.try_get(8)?,
        issue_prefix: row.try_get(9)?,
        issue_counter: row.try_get(10)?,
        avatar_url: row.try_get(11)?,
        attribution_fail_closed: row.try_get(12)?,
    }))
}

pub async fn delete_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH ws_installations AS (
    SELECT id FROM channel_installation WHERE workspace_id = $1
),
ws_agents AS (
    SELECT id FROM agent WHERE workspace_id = $1
),
ws_skills AS (
    SELECT id FROM skill WHERE workspace_id = $1
),
cleared_agent_label_assignments AS (
    DELETE FROM agent_to_label WHERE agent_id IN (SELECT id FROM ws_agents)
),
cleared_skill_label_assignments AS (
    DELETE FROM skill_to_label WHERE skill_id IN (SELECT id FROM ws_skills)
),
cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding WHERE installation_id IN (SELECT id FROM ws_installations)
    RETURNING chat_session_id
),
cleared_dingtalk_group_routes AS (
    DELETE FROM dingtalk_group_route WHERE workspace_id = $1
),
cleared_outbound_cards AS (
    -- channel_outbound_card_message is keyed by chat_session_id (no FK); its own
    -- chat_session rows cascade away with the workspace, so reach the cards through
    -- the just-removed chat-session bindings, which still carry the id.
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
),
cleared_draft_restores AS (
    -- chat_draft_restore is keyed by chat_session_id with no FK (MUL-3515) and has
    -- no reaper, while its chat_session rows cascade away with the workspace. Reach
    -- them directly through chat_session (unlike the cards above, this is not
    -- limited to channel-bound sessions) or every pending restore — each holding a
    -- user's prompt text — would outlive the workspace permanently (#5219).
    --
    -- This sweep only sees restores committed before the statement's snapshot, so
    -- the caller must already hold LockChatSessionsByWorkspace: that lock is what
    -- keeps FinalizeDeferredCancelledChat from inserting one behind it.
    DELETE FROM chat_draft_restore
    WHERE chat_session_id IN (SELECT id FROM chat_session WHERE workspace_id = $1)
),
cleared_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup WHERE installation_id IN (SELECT id FROM ws_installations)
),
cleared_audit AS (
    -- Purge, don't detach: the workspace is gone and channel_inbound_audit has no
    -- workspace_id and no reaper, so a detached (NULL) row would be permanently
    -- unattributable. (Reclaim, where the workspace survives, still detaches.)
    DELETE FROM channel_inbound_audit WHERE installation_id IN (SELECT id FROM ws_installations)
),
cleared_user_bindings AS (
    DELETE FROM channel_user_binding WHERE workspace_id = $1
),
cleared_binding_tokens AS (
    DELETE FROM channel_binding_token WHERE workspace_id = $1
),
cleared_installations AS (
    DELETE FROM channel_installation WHERE workspace_id = $1
),
cleared_issue_properties AS (
    DELETE FROM issue_property WHERE workspace_id = $1
),
cleared_quick_actions AS (
    DELETE FROM quick_action WHERE workspace_id = $1
),
ws_mcp_servers AS (
    SELECT id FROM workspace_mcp_server WHERE workspace_id = $1
),
cleared_agent_mcp_bindings AS (
    -- agent_mcp_server carries no FK in either direction, so sweep it from
    -- both sides: the workspace's own servers, and any binding held by an
    -- agent that is about to be removed with the workspace.
    DELETE FROM agent_mcp_server
    WHERE server_id IN (SELECT id FROM ws_mcp_servers)
       OR agent_id IN (SELECT id FROM ws_agents)
),
cleared_workspace_mcp_servers AS (
    DELETE FROM workspace_mcp_server WHERE workspace_id = $1
),
deleted_pending_check_suites AS (
    DELETE FROM github_pending_check_suite WHERE workspace_id = $1
),
ws_github_prs AS (
    SELECT id FROM github_pull_request WHERE workspace_id = $1
),
cleared_github_pr_check_runs AS (
    -- github_pull_request_check_run intentionally has no FK. Remove its rows
    -- before the workspace delete cascades away the parent PR mirrors.
    DELETE FROM github_pull_request_check_run
    WHERE pr_id IN (SELECT id FROM ws_github_prs)
),
ws_vcs_prs AS (
    SELECT id FROM vcs_pull_request WHERE workspace_id = $1
),
ws_vcs_connections AS (
    SELECT id FROM vcs_connection WHERE workspace_id = $1
),
cleared_vcs_pr_links AS (
    DELETE FROM issue_vcs_pull_request
    WHERE pull_request_id IN (SELECT id FROM ws_vcs_prs)
),
cleared_vcs_commit_statuses AS (
    DELETE FROM vcs_commit_status
    WHERE connection_id IN (SELECT id FROM ws_vcs_connections)
),
cleared_vcs_prs AS (
    DELETE FROM vcs_pull_request WHERE workspace_id = $1
),
cleared_vcs_connections AS (
    DELETE FROM vcs_connection WHERE workspace_id = $1
),
cleared_client_usage_workspace AS (
    UPDATE client_usage_daily SET workspace_id = NULL WHERE workspace_id = $1
)
DELETE FROM workspace WHERE workspace.id = $1"#
    )
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetDaemonWorkspaceRow {
    pub id: Option<Uuid>,
    pub name: String,
}

pub async fn get_daemon_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<GetDaemonWorkspaceRow>> {
    let row = sqlx::query(
        r#"SELECT id, name
FROM workspace
WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetDaemonWorkspaceRow {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
    }))
}

pub async fn get_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Workspace>> {
    let row = sqlx::query(
        r#"SELECT id, name, slug, description, settings, created_at, updated_at, context, repos, issue_prefix, issue_counter, avatar_url, attribution_fail_closed FROM workspace
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Workspace {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        slug: row.try_get(2)?,
        description: row.try_get(3)?,
        settings: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
        context: row.try_get(7)?,
        repos: row.try_get(8)?,
        issue_prefix: row.try_get(9)?,
        issue_counter: row.try_get(10)?,
        avatar_url: row.try_get(11)?,
        attribution_fail_closed: row.try_get(12)?,
    }))
}

pub async fn get_workspace_attribution_fail_closed(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT attribution_fail_closed FROM workspace
WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn get_workspace_by_slug(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    slug: &str,
) -> anyhow::Result<Option<Workspace>> {
    let row = sqlx::query(
        r#"SELECT id, name, slug, description, settings, created_at, updated_at, context, repos, issue_prefix, issue_counter, avatar_url, attribution_fail_closed FROM workspace
WHERE slug = $1"#
    )
        .bind(slug)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Workspace {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        slug: row.try_get(2)?,
        description: row.try_get(3)?,
        settings: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
        context: row.try_get(7)?,
        repos: row.try_get(8)?,
        issue_prefix: row.try_get(9)?,
        issue_counter: row.try_get(10)?,
        avatar_url: row.try_get(11)?,
        attribution_fail_closed: row.try_get(12)?,
    }))
}

pub async fn increment_issue_counter(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<i32>> {
    let row = sqlx::query(
        r#"UPDATE workspace SET issue_counter = issue_counter + 1
WHERE id = $1
RETURNING issue_counter"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDaemonWorkspacesRow {
    pub id: Option<Uuid>,
    pub name: String,
}

pub async fn list_daemon_workspaces(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Vec<ListDaemonWorkspacesRow>> {
    let rows = sqlx::query(
        r#"SELECT w.id, w.name
FROM member m
JOIN workspace w ON w.id = m.workspace_id
WHERE m.user_id = $1
ORDER BY w.id ASC"#,
    )
    .bind(user_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDaemonWorkspacesRow {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn list_workspaces(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Vec<Workspace>> {
    let rows = sqlx::query(
        r#"SELECT w.id, w.name, w.slug, w.description, w.settings,
       w.created_at, w.updated_at, w.context, w.repos,
       w.issue_prefix, w.issue_counter, w.avatar_url, w.attribution_fail_closed
FROM member m
JOIN workspace w ON w.id = m.workspace_id
WHERE m.user_id = $1
ORDER BY w.created_at ASC"#,
    )
    .bind(user_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Workspace {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
            slug: row.try_get(2)?,
            description: row.try_get(3)?,
            settings: row.try_get(4)?,
            created_at: row.try_get(5)?,
            updated_at: row.try_get(6)?,
            context: row.try_get(7)?,
            repos: row.try_get(8)?,
            issue_prefix: row.try_get(9)?,
            issue_counter: row.try_get(10)?,
            avatar_url: row.try_get(11)?,
            attribution_fail_closed: row.try_get(12)?,
        });
    }
    Ok(out)
}

pub async fn lock_workspace_for_chat_session_create(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(r#"SELECT id FROM workspace WHERE id = $1 FOR KEY SHARE"#)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_workspace_for_delete(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(r#"SELECT id FROM workspace WHERE id = $1 FOR UPDATE"#)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn update_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    context: Option<&str>,
    settings: &serde_json::Value,
    repos: &serde_json::Value,
    issue_prefix: Option<&str>,
    avatar_url: Option<&str>,
) -> anyhow::Result<Option<Workspace>> {
    let row = sqlx::query(
        r#"UPDATE workspace SET
    name = COALESCE($2, name),
    description = COALESCE($3, description),
    context = COALESCE($4, context),
    settings = COALESCE($5, settings),
    repos = COALESCE($6, repos),
    issue_prefix = COALESCE($7, issue_prefix),
    avatar_url = COALESCE($8, avatar_url),
    updated_at = now()
WHERE id = $1
RETURNING id, name, slug, description, settings, created_at, updated_at, context, repos, issue_prefix, issue_counter, avatar_url, attribution_fail_closed"#
    )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(context)
        .bind(settings)
        .bind(repos)
        .bind(issue_prefix)
        .bind(avatar_url)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Workspace {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        slug: row.try_get(2)?,
        description: row.try_get(3)?,
        settings: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
        context: row.try_get(7)?,
        repos: row.try_get(8)?,
        issue_prefix: row.try_get(9)?,
        issue_counter: row.try_get(10)?,
        avatar_url: row.try_get(11)?,
        attribution_fail_closed: row.try_get(12)?,
    }))
}
