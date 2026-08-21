//! Port of server/pkg/db/queries/squad.sql (generated squad.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn add_squad_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
    member_type: &str,
    member_id: Uuid,
    role: &str,
) -> anyhow::Result<Option<SquadMember>> {
    let row = sqlx::query(
        r#"INSERT INTO squad_member (squad_id, member_type, member_id, role)
VALUES ($1, $2, $3, $4)
RETURNING id, squad_id, member_type, member_id, role, created_at"#,
    )
    .bind(squad_id)
    .bind(member_type)
    .bind(member_id)
    .bind(role)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SquadMember {
        id: row.try_get(0)?,
        squad_id: row.try_get(1)?,
        member_type: row.try_get(2)?,
        member_id: row.try_get(3)?,
        role: row.try_get(4)?,
        created_at: row.try_get(5)?,
    }))
}

pub async fn archive_squad(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    archived_by: Uuid,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"UPDATE squad SET archived_at = now(), archived_by = $2, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions"#
    )
        .bind(id)
        .bind(archived_by)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn count_squad_members(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(r#"SELECT count(*) FROM squad_member WHERE squad_id = $1"#)
        .bind(squad_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_squad(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &str,
    leader_id: Uuid,
    creator_id: Uuid,
    avatar_url: Option<&str>,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"INSERT INTO squad (workspace_id, name, description, leader_id, creator_id, avatar_url)
VALUES ($1, $2, $3, $4, $5, $6)
RETURNING id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(leader_id)
        .bind(creator_id)
        .bind(avatar_url)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn get_squad(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions FROM squad WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn get_squad_by_assignee(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"SELECT s.id, s.workspace_id, s.name, s.description, s.leader_id, s.creator_id, s.created_at, s.updated_at, s.archived_at, s.archived_by, s.avatar_url, s.instructions FROM squad s WHERE s.id = $1 AND s.workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn get_squad_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions FROM squad WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn is_squad_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
    member_type: &str,
    member_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS(
    SELECT 1 FROM squad_member
    WHERE squad_id = $1 AND member_type = $2 AND member_id = $3
) AS is_member"#,
    )
    .bind(squad_id)
    .bind(member_type)
    .bind(member_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_all_squads(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Squad>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions FROM squad WHERE workspace_id = $1 ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Squad {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            leader_id: row.try_get(4)?,
            creator_id: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            archived_at: row.try_get(8)?,
            archived_by: row.try_get(9)?,
            avatar_url: row.try_get(10)?,
            instructions: row.try_get(11)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListSquadMemberPreviewRowsRow {
    pub squad_id: Option<Uuid>,
    pub member_type: String,
    pub member_id: Option<Uuid>,
    pub role: String,
}

pub async fn list_squad_member_preview_rows(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListSquadMemberPreviewRowsRow>> {
    let rows = sqlx::query(
        r#"SELECT
    sm.squad_id,
    sm.member_type,
    sm.member_id,
    sm.role
FROM squad_member sm
JOIN squad s ON s.id = sm.squad_id
WHERE s.workspace_id = $1 AND s.archived_at IS NULL
ORDER BY
    sm.squad_id ASC,
    (sm.member_type = 'agent' AND sm.member_id = s.leader_id) DESC,
    sm.created_at ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListSquadMemberPreviewRowsRow {
            squad_id: row.try_get(0)?,
            member_type: row.try_get(1)?,
            member_id: row.try_get(2)?,
            role: row.try_get(3)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListSquadMemberPreviewRowsBySquadRow {
    pub squad_id: Option<Uuid>,
    pub member_type: String,
    pub member_id: Option<Uuid>,
    pub role: String,
}

pub async fn list_squad_member_preview_rows_by_squad(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
) -> anyhow::Result<Vec<ListSquadMemberPreviewRowsBySquadRow>> {
    let rows = sqlx::query(
        r#"SELECT
    sm.squad_id,
    sm.member_type,
    sm.member_id,
    sm.role
FROM squad_member sm
JOIN squad s ON s.id = sm.squad_id
WHERE sm.squad_id = $1
ORDER BY
    (sm.member_type = 'agent' AND sm.member_id = s.leader_id) DESC,
    sm.created_at ASC"#,
    )
    .bind(squad_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListSquadMemberPreviewRowsBySquadRow {
            squad_id: row.try_get(0)?,
            member_type: row.try_get(1)?,
            member_id: row.try_get(2)?,
            role: row.try_get(3)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListSquadMemberStatusRowsRow {
    pub squad_member_id: Option<Uuid>,
    pub member_type: String,
    pub member_id: Option<Uuid>,
    pub agent_archived_at: Option<DateTime<Utc>>,
    pub runtime_status: Option<String>,
    pub runtime_last_seen_at: Option<DateTime<Utc>>,
    pub task_id: Option<Uuid>,
    pub task_status: Option<String>,
    pub task_issue_id: Option<Uuid>,
    pub task_dispatched_at: Option<DateTime<Utc>>,
    pub issue_number: Option<i32>,
    pub issue_title: Option<String>,
    pub issue_status: Option<String>,
}

pub async fn list_squad_member_status_rows(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
) -> anyhow::Result<Vec<ListSquadMemberStatusRowsRow>> {
    let rows = sqlx::query(
        r#"SELECT
    sm.id              AS squad_member_id,
    sm.member_type     AS member_type,
    sm.member_id       AS member_id,
    a.archived_at      AS agent_archived_at,
    ar.status          AS runtime_status,
    ar.last_seen_at    AS runtime_last_seen_at,
    atq.id             AS task_id,
    atq.status         AS task_status,
    atq.issue_id       AS task_issue_id,
    atq.dispatched_at  AS task_dispatched_at,
    i.number           AS issue_number,
    i.title            AS issue_title,
    i.status           AS issue_status
FROM squad_member sm
LEFT JOIN agent a
       ON sm.member_type = 'agent' AND a.id = sm.member_id
LEFT JOIN agent_runtime ar
       ON ar.id = a.runtime_id
LEFT JOIN agent_task_queue atq
       ON sm.member_type = 'agent'
      AND atq.agent_id = sm.member_id
      AND atq.status IN ('dispatched', 'running', 'waiting_local_directory')
LEFT JOIN issue i
       ON i.id = atq.issue_id
WHERE sm.squad_id = $1
ORDER BY sm.created_at ASC, atq.dispatched_at DESC NULLS LAST"#,
    )
    .bind(squad_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListSquadMemberStatusRowsRow {
            squad_member_id: row.try_get(0)?,
            member_type: row.try_get(1)?,
            member_id: row.try_get(2)?,
            agent_archived_at: row.try_get(3)?,
            runtime_status: row.try_get(4)?,
            runtime_last_seen_at: row.try_get(5)?,
            task_id: row.try_get(6)?,
            task_status: row.try_get(7)?,
            task_issue_id: row.try_get(8)?,
            task_dispatched_at: row.try_get(9)?,
            issue_number: row.try_get(10)?,
            issue_title: row.try_get(11)?,
            issue_status: row.try_get(12)?,
        });
    }
    Ok(out)
}

pub async fn list_squad_members(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
) -> anyhow::Result<Vec<SquadMember>> {
    let rows = sqlx::query(
        r#"SELECT id, squad_id, member_type, member_id, role, created_at FROM squad_member WHERE squad_id = $1 ORDER BY created_at ASC"#
    )
        .bind(squad_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(SquadMember {
            id: row.try_get(0)?,
            squad_id: row.try_get(1)?,
            member_type: row.try_get(2)?,
            member_id: row.try_get(3)?,
            role: row.try_get(4)?,
            created_at: row.try_get(5)?,
        });
    }
    Ok(out)
}

pub async fn list_squads(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Squad>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions FROM squad WHERE workspace_id = $1 AND archived_at IS NULL ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Squad {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            leader_id: row.try_get(4)?,
            creator_id: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            archived_at: row.try_get(8)?,
            archived_by: row.try_get(9)?,
            avatar_url: row.try_get(10)?,
            instructions: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_squads_by_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    member_type: &str,
    member_id: Uuid,
) -> anyhow::Result<Vec<Squad>> {
    let rows = sqlx::query(
        r#"SELECT s.id, s.workspace_id, s.name, s.description, s.leader_id, s.creator_id, s.created_at, s.updated_at, s.archived_at, s.archived_by, s.avatar_url, s.instructions FROM squad s
JOIN squad_member sm ON sm.squad_id = s.id
WHERE s.workspace_id = $1 AND sm.member_type = $2 AND sm.member_id = $3
ORDER BY s.created_at ASC"#
    )
        .bind(workspace_id)
        .bind(member_type)
        .bind(member_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Squad {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            leader_id: row.try_get(4)?,
            creator_id: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            archived_at: row.try_get(8)?,
            archived_by: row.try_get(9)?,
            avatar_url: row.try_get(10)?,
            instructions: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn lock_squad_for_autopilot_assignment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions FROM squad
WHERE id = $1 AND workspace_id = $2
FOR SHARE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn lock_squad_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions FROM squad
WHERE id = $1 AND workspace_id = $2
FOR UPDATE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn remove_squad_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
    member_type: &str,
    member_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM squad_member
WHERE squad_id = $1 AND member_type = $2 AND member_id = $3"#,
    )
    .bind(squad_id)
    .bind(member_type)
    .bind(member_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn transfer_squad_assignees(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    assignee_id: Uuid,
    assignee_id_2: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE issue SET assignee_type = 'agent', assignee_id = $2, revision = revision + 1, updated_at = now()
WHERE assignee_type = 'squad' AND assignee_id = $1"#
    )
        .bind(assignee_id)
        .bind(assignee_id_2)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn transfer_squad_autopilots_to_leader(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    assignee_id: Uuid,
    assignee_id_2: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE autopilot
SET assignee_type = 'agent',
    assignee_id = $2,
    updated_at = now()
WHERE assignee_type = 'squad' AND assignee_id = $1"#,
    )
    .bind(assignee_id)
    .bind(assignee_id_2)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_squad(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    leader_id: Uuid,
    avatar_url: Option<&str>,
    instructions: Option<&str>,
) -> anyhow::Result<Option<Squad>> {
    let row = sqlx::query(
        r#"UPDATE squad SET
    name = COALESCE($2, name),
    description = COALESCE($3, description),
    leader_id = COALESCE($4, leader_id),
    avatar_url = COALESCE($5, avatar_url),
    instructions = COALESCE($6, instructions),
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, description, leader_id, creator_id, created_at, updated_at, archived_at, archived_by, avatar_url, instructions"#
    )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(leader_id)
        .bind(avatar_url)
        .bind(instructions)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Squad {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        leader_id: row.try_get(4)?,
        creator_id: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        archived_at: row.try_get(8)?,
        archived_by: row.try_get(9)?,
        avatar_url: row.try_get(10)?,
        instructions: row.try_get(11)?,
    }))
}

pub async fn update_squad_member_role(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    squad_id: Uuid,
    member_type: &str,
    member_id: Uuid,
    role: &str,
) -> anyhow::Result<Option<SquadMember>> {
    let row = sqlx::query(
        r#"UPDATE squad_member SET role = $4
WHERE squad_id = $1 AND member_type = $2 AND member_id = $3
RETURNING id, squad_id, member_type, member_id, role, created_at"#,
    )
    .bind(squad_id)
    .bind(member_type)
    .bind(member_id)
    .bind(role)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SquadMember {
        id: row.try_get(0)?,
        squad_id: row.try_get(1)?,
        member_type: row.try_get(2)?,
        member_id: row.try_get(3)?,
        role: row.try_get(4)?,
        created_at: row.try_get(5)?,
    }))
}
