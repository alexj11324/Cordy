//! Typed SQL queries for dingtalk records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn delete_ding_talk_installation_for_replacement(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"WITH retired AS (
    DELETE FROM channel_installation ci
    WHERE ci.id = $1
      AND ci.workspace_id = $2
      AND ci.agent_id = $3
      AND ci.channel_type = 'dingtalk'
    RETURNING ci.id
),
cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding
    WHERE installation_id IN (SELECT id FROM retired)
    RETURNING chat_session_id
),
cleared_group_routes AS (
    DELETE FROM dingtalk_group_route
    WHERE installation_id IN (SELECT id FROM retired)
),
cleared_outbound_cards AS (
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
),
cleared_binding_tokens AS (
    DELETE FROM channel_binding_token
    WHERE installation_id IN (SELECT id FROM retired)
),
cleared_user_bindings AS (
    DELETE FROM channel_user_binding
    WHERE installation_id IN (SELECT id FROM retired)
),
cleared_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup
    WHERE installation_id IN (SELECT id FROM retired)
),
detached_audit AS (
    UPDATE channel_inbound_audit SET installation_id = NULL
    WHERE installation_id IN (SELECT id FROM retired)
),
detached_media_intents AS (
    UPDATE channel_media_pending_object SET installation_id = NULL
    WHERE installation_id IN (SELECT id FROM retired)
)
SELECT retired.id FROM retired"#,
    )
    .bind(installation_id)
    .bind(workspace_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn delete_ding_talk_stale_group_chat_binding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    conversation_id: &str,
    agent_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"WITH cleared AS (
    DELETE FROM channel_chat_session_binding b
    WHERE b.installation_id = $1
      AND b.channel_type = 'dingtalk'
      AND b.channel_chat_id = $2::text
      AND COALESCE(
          NULLIF(b.config ->> 'agent_id', ''),
          (
              SELECT i.agent_id::text
              FROM channel_installation i
              WHERE i.id = b.installation_id
          ),
          ''
      ) <> $3::uuid::text
    RETURNING b.chat_session_id
), cleared_outbound_cards AS (
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared)
)
SELECT count(*)::bigint AS cleared_count
FROM cleared"#,
    )
    .bind(installation_id)
    .bind(conversation_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn ding_talk_group_route_matches_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    conversation_id: &str,
    agent_id: Uuid,
    route_revision: i64,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
    SELECT 1
    FROM dingtalk_group_route r
    JOIN agent a ON a.id = r.agent_id
                AND a.workspace_id = r.workspace_id
    WHERE r.installation_id = $1
      AND r.conversation_id = $2::text
      AND r.agent_id = $3
      AND r.revision = $4
      AND a.kind = 'user'
      AND a.archived_at IS NULL
) AS matches"#,
    )
    .bind(installation_id)
    .bind(conversation_id)
    .bind(agent_id)
    .bind(route_revision)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoverDingTalkGroupRouteRow {
    pub agent_id: Option<Uuid>,
    pub revision: i64,
    pub agent_active: bool,
}

pub async fn discover_ding_talk_group_route(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    installation_id: Uuid,
    conversation_id: &str,
    conversation_title: &str,
) -> anyhow::Result<Option<DiscoverDingTalkGroupRouteRow>> {
    let row = sqlx::query(
        r#"WITH workspace_guard AS MATERIALIZED (
    SELECT w.id
    FROM workspace w
    WHERE w.id = $1
    FOR KEY SHARE
), installation AS MATERIALIZED (
    SELECT i.id, i.workspace_id, i.agent_id, i.channel_type, i.config, i.status, i.ws_lease_token, i.ws_lease_expires_at, i.installer_user_id, i.installed_at, i.created_at, i.updated_at
    FROM channel_installation i
    JOIN workspace_guard w ON w.id = i.workspace_id
    WHERE i.id = $2
      AND i.workspace_id = $1
      AND i.channel_type = 'dingtalk'
      AND i.status = 'active'
    FOR SHARE OF i
), group_route AS (
    INSERT INTO dingtalk_group_route (
        workspace_id, installation_id, conversation_id,
        conversation_title, agent_id
    )
    SELECT
        i.workspace_id, i.id, $3::text,
        $4::text, i.agent_id
    FROM installation i
    ON CONFLICT (installation_id, conversation_id) DO UPDATE SET
        conversation_title = CASE
            WHEN EXCLUDED.conversation_title = '' THEN dingtalk_group_route.conversation_title
            ELSE EXCLUDED.conversation_title
        END,
        updated_at = CASE
            WHEN EXCLUDED.conversation_title <> ''
             AND EXCLUDED.conversation_title IS DISTINCT FROM dingtalk_group_route.conversation_title
                THEN now()
            ELSE dingtalk_group_route.updated_at
        END
    RETURNING agent_id, workspace_id, revision
)
SELECT r.agent_id,
       r.revision,
       EXISTS (
           SELECT 1 FROM agent a
           WHERE a.id = r.agent_id
             AND a.workspace_id = r.workspace_id
             AND a.kind = 'user'
             AND a.archived_at IS NULL
       ) AS agent_active
FROM group_route r"#
    )
        .bind(workspace_id)
        .bind(installation_id)
        .bind(conversation_id)
        .bind(conversation_title)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DiscoverDingTalkGroupRouteRow {
        agent_id: row.try_get(0)?,
        revision: row.try_get(1)?,
        agent_active: row.try_get(2)?,
    }))
}

pub async fn get_ding_talk_group_route_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<DingtalkGroupRoute>> {
    let row = sqlx::query(
        r#"SELECT r.id, r.workspace_id, r.installation_id, r.conversation_id, r.conversation_title, r.agent_id, r.revision, r.discovered_at, r.updated_at
FROM dingtalk_group_route r
JOIN channel_installation i ON i.id = r.installation_id
WHERE r.id = $1
  AND r.workspace_id = $2
  AND i.workspace_id = $2
  AND i.channel_type = 'dingtalk'
  AND i.status = 'active'"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DingtalkGroupRoute {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        conversation_id: row.try_get(3)?,
        conversation_title: row.try_get(4)?,
        agent_id: row.try_get(5)?,
        revision: row.try_get(6)?,
        discovered_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetDingTalkInstallationOwnerForUpdateRow {
    pub id: Option<Uuid>,
    pub app_id: String,
}

pub async fn get_ding_talk_installation_owner_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<GetDingTalkInstallationOwnerForUpdateRow>> {
    let row = sqlx::query(
        r#"SELECT id, COALESCE(config ->> 'app_id', '')::text AS app_id
FROM channel_installation
WHERE workspace_id = $1
  AND agent_id = $2
  AND channel_type = 'dingtalk'
FOR UPDATE"#,
    )
    .bind(workspace_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetDingTalkInstallationOwnerForUpdateRow {
        id: row.try_get(0)?,
        app_id: row.try_get(1)?,
    }))
}

pub async fn list_ding_talk_group_routes_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<DingtalkGroupRoute>> {
    let rows = sqlx::query(
        r#"SELECT r.id, r.workspace_id, r.installation_id, r.conversation_id, r.conversation_title, r.agent_id, r.revision, r.discovered_at, r.updated_at
FROM dingtalk_group_route r
JOIN channel_installation i ON i.id = r.installation_id
WHERE r.workspace_id = $1
  AND i.workspace_id = $1
  AND i.channel_type = 'dingtalk'
  AND i.status = 'active'
ORDER BY r.discovered_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(DingtalkGroupRoute {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            installation_id: row.try_get(2)?,
            conversation_id: row.try_get(3)?,
            conversation_title: row.try_get(4)?,
            agent_id: row.try_get(5)?,
            revision: row.try_get(6)?,
            discovered_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDingTalkUserBindingsForMemberRow {
    pub installation_id: Option<Uuid>,
    pub channel_user_id: String,
}

pub async fn list_ding_talk_user_bindings_for_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    cordy_user_id: Uuid,
) -> anyhow::Result<Vec<ListDingTalkUserBindingsForMemberRow>> {
    let rows = sqlx::query(
        r#"SELECT installation_id, channel_user_id
FROM channel_user_binding
WHERE workspace_id = $1
  AND cordy_user_id = $2
  AND channel_type = 'dingtalk'
ORDER BY bound_at DESC, id ASC"#,
    )
    .bind(workspace_id)
    .bind(cordy_user_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDingTalkUserBindingsForMemberRow {
            installation_id: row.try_get(0)?,
            channel_user_id: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn lock_ding_talk_group_route_for_append(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    conversation_id: &str,
    agent_id: Uuid,
    route_revision: i64,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"WITH target_agent AS MATERIALIZED (
    SELECT a.id
    FROM agent a
    WHERE a.id = $3
      AND a.kind = 'user'
      AND a.archived_at IS NULL
    FOR SHARE
)
SELECT r.revision
FROM dingtalk_group_route r
WHERE r.installation_id = $1
  AND r.conversation_id = $2::text
  AND r.agent_id = $3
  AND r.revision = $4
  AND EXISTS (SELECT 1 FROM target_agent)
FOR SHARE OF r"#,
    )
    .bind(installation_id)
    .bind(conversation_id)
    .bind(agent_id)
    .bind(route_revision)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_ding_talk_installation_owner(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT pg_advisory_xact_lock(
    hashtextextended(
        ($1::uuid)::text || ':' ||
        ($2::uuid)::text || ':dingtalk',
        0
    )
)"#,
    )
    .bind(workspace_id)
    .bind(agent_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReassignDingTalkGroupRouteRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub installation_id: Option<Uuid>,
    pub conversation_id: String,
    pub conversation_title: String,
    pub agent_id: Option<Uuid>,
    pub revision: i64,
    pub discovered_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn reassign_ding_talk_group_route(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<ReassignDingTalkGroupRouteRow>> {
    let row = sqlx::query(
        r#"WITH workspace_guard AS MATERIALIZED (
    SELECT w.id
    FROM workspace w
    WHERE w.id = $1
    FOR KEY SHARE
), target_agent AS MATERIALIZED (
    SELECT a.id
    FROM agent a
    JOIN workspace_guard w ON w.id = a.workspace_id
    WHERE a.id = $2
      AND a.workspace_id = $1
      AND a.kind = 'user'
      AND a.archived_at IS NULL
    FOR SHARE
), active_installation AS MATERIALIZED (
    SELECT i.id
    FROM channel_installation i
    JOIN dingtalk_group_route r ON r.installation_id = i.id
    WHERE r.id = $3
      AND r.workspace_id = $1
      AND i.workspace_id = $1
      AND i.channel_type = 'dingtalk'
      AND i.status = 'active'
      AND EXISTS (SELECT 1 FROM target_agent)
    FOR SHARE OF i
), target AS (
    SELECT r.id, r.workspace_id, r.installation_id, r.conversation_id, r.conversation_title, r.agent_id, r.revision, r.discovered_at, r.updated_at, r.agent_id AS previous_agent_id
    FROM dingtalk_group_route r
    JOIN active_installation i ON i.id = r.installation_id
    WHERE r.id = $3
      AND r.workspace_id = $1
    FOR UPDATE OF r
), updated AS (
    UPDATE dingtalk_group_route r
    SET agent_id = $2,
        revision = r.revision + 1,
        updated_at = now()
    FROM target t
    WHERE r.id = t.id
    RETURNING r.id, r.workspace_id, r.installation_id, r.conversation_id, r.conversation_title, r.agent_id, r.revision, r.discovered_at, r.updated_at, t.previous_agent_id
), cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding b
    USING updated u
    WHERE u.previous_agent_id IS DISTINCT FROM u.agent_id
      AND b.installation_id = u.installation_id
      AND b.channel_chat_id = u.conversation_id
    RETURNING b.chat_session_id
), cleared_outbound_cards AS (
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
)
SELECT id, workspace_id, installation_id, conversation_id,
       conversation_title, agent_id, revision, discovered_at, updated_at
FROM updated"#
    )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ReassignDingTalkGroupRouteRow {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        conversation_id: row.try_get(3)?,
        conversation_title: row.try_get(4)?,
        agent_id: row.try_get(5)?,
        revision: row.try_get(6)?,
        discovered_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
    }))
}
