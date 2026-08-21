//! Port of server/pkg/db/queries/skill.sql (generated skill.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn add_agent_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    skill_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO agent_skill (agent_id, skill_id)
VALUES ($1, $2)
ON CONFLICT DO NOTHING"#,
    )
    .bind(agent_id)
    .bind(skill_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn create_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &str,
    content: &str,
    config: &serde_json::Value,
    created_by: Uuid,
) -> anyhow::Result<Option<Skill>> {
    let row = sqlx::query(
        r#"INSERT INTO skill (workspace_id, name, description, content, config, created_by)
VALUES ($1, $2, $3, $4, $5, $6)
RETURNING id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(content)
        .bind(config)
        .bind(created_by)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Skill {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        content: row.try_get(4)?,
        config: row.try_get(5)?,
        created_by: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        plugin_installation_id: row.try_get(9)?,
    }))
}

pub async fn delete_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM skill WHERE id = $1 AND workspace_id = $2"#)
        .bind(id)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_skill_file(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM skill_file WHERE id = $1"#)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_skill_files_by_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM skill_file WHERE skill_id = $1"#)
        .bind(skill_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Skill>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id FROM skill
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Skill {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        content: row.try_get(4)?,
        config: row.try_get(5)?,
        created_by: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        plugin_installation_id: row.try_get(9)?,
    }))
}

pub async fn get_skill_by_workspace_and_name(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
) -> anyhow::Result<Option<Skill>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id FROM skill
WHERE workspace_id = $1 AND name = $2"#
    )
        .bind(workspace_id)
        .bind(name)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Skill {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        content: row.try_get(4)?,
        config: row.try_get(5)?,
        created_by: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        plugin_installation_id: row.try_get(9)?,
    }))
}

pub async fn get_skill_file(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<SkillFile>> {
    let row = sqlx::query(
        r#"SELECT id, skill_id, path, content, created_at, updated_at FROM skill_file
WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SkillFile {
        id: row.try_get(0)?,
        skill_id: row.try_get(1)?,
        path: row.try_get(2)?,
        content: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}

pub async fn get_skill_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Skill>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id FROM skill
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Skill {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        content: row.try_get(4)?,
        config: row.try_get(5)?,
        created_by: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        plugin_installation_id: row.try_get(9)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAgentSkillNamesByAgentIDsRow {
    pub agent_id: Option<Uuid>,
    pub name: String,
}

pub async fn list_agent_skill_names_by_agent_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<ListAgentSkillNamesByAgentIDsRow>> {
    let rows = sqlx::query(
        r#"SELECT ask.agent_id, s.name
FROM agent_skill ask
JOIN skill s ON s.id = ask.skill_id
WHERE ask.agent_id = ANY($1::uuid[])
  AND ask.enabled = TRUE
ORDER BY ask.agent_id, s.name ASC"#,
    )
    .bind(agent_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAgentSkillNamesByAgentIDsRow {
            agent_id: row.try_get(0)?,
            name: row.try_get(1)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAgentSkillSummariesRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub config: Option<serde_json::Value>,
    pub created_by: Option<Uuid>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub enabled: bool,
}

pub async fn list_agent_skill_summaries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<ListAgentSkillSummariesRow>> {
    let rows = sqlx::query(
        r#"SELECT s.id, s.workspace_id, s.name, s.description, s.config, s.created_by, s.created_at, s.updated_at, ask.enabled
FROM skill s
JOIN agent_skill ask ON ask.skill_id = s.id
WHERE ask.agent_id = $1
ORDER BY s.name ASC"#
    )
        .bind(agent_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAgentSkillSummariesRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            config: row.try_get(4)?,
            created_by: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            enabled: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_agent_skills(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<Skill>> {
    let rows = sqlx::query(
        r#"SELECT s.id, s.workspace_id, s.name, s.description, s.content, s.config, s.created_by, s.created_at, s.updated_at, s.plugin_installation_id FROM skill s
JOIN agent_skill ask ON ask.skill_id = s.id
WHERE ask.agent_id = $1 AND ask.enabled = TRUE
ORDER BY s.name ASC"#
    )
        .bind(agent_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Skill {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            content: row.try_get(4)?,
            config: row.try_get(5)?,
            created_by: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
            plugin_installation_id: row.try_get(9)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAgentSkillsByWorkspaceRow {
    pub agent_id: Option<Uuid>,
    pub id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

pub async fn list_agent_skills_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListAgentSkillsByWorkspaceRow>> {
    let rows = sqlx::query(
        r#"SELECT ask.agent_id, s.id, s.name, s.description, ask.enabled
FROM agent_skill ask
JOIN skill s ON s.id = ask.skill_id
WHERE s.workspace_id = $1
ORDER BY s.name ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAgentSkillsByWorkspaceRow {
            agent_id: row.try_get(0)?,
            id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            enabled: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn list_skill_files(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
) -> anyhow::Result<Vec<SkillFile>> {
    let rows = sqlx::query(
        r#"SELECT id, skill_id, path, content, created_at, updated_at FROM skill_file
WHERE skill_id = $1
ORDER BY path ASC"#,
    )
    .bind(skill_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(SkillFile {
            id: row.try_get(0)?,
            skill_id: row.try_get(1)?,
            path: row.try_get(2)?,
            content: row.try_get(3)?,
            created_at: row.try_get(4)?,
            updated_at: row.try_get(5)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListSkillSummariesByWorkspaceRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub config: Option<serde_json::Value>,
    pub created_by: Option<Uuid>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn list_skill_summaries_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListSkillSummariesByWorkspaceRow>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, description, config, created_by, created_at, updated_at
FROM skill
WHERE workspace_id = $1
ORDER BY name ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListSkillSummariesByWorkspaceRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            config: row.try_get(4)?,
            created_by: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn list_skills_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Skill>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id FROM skill
WHERE workspace_id = $1
ORDER BY name ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Skill {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            content: row.try_get(4)?,
            config: row.try_get(5)?,
            created_by: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
            plugin_installation_id: row.try_get(9)?,
        });
    }
    Ok(out)
}

pub async fn remove_agent_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    skill_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_skill
WHERE agent_id = $1 AND skill_id = $2"#,
    )
    .bind(agent_id)
    .bind(skill_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn remove_all_agent_skills(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent_skill WHERE agent_id = $1"#)
        .bind(agent_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn set_agent_skill_enabled(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    skill_id: Uuid,
    enabled: bool,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_skill
SET enabled = $3
WHERE agent_id = $1 AND skill_id = $2"#,
    )
    .bind(agent_id)
    .bind(skill_id)
    .bind(enabled)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    content: Option<&str>,
    config: &serde_json::Value,
) -> anyhow::Result<Option<Skill>> {
    let row = sqlx::query(
        r#"UPDATE skill SET
    name = COALESCE($2, name),
    description = COALESCE($3, description),
    content = COALESCE($4, content),
    config = COALESCE($5, config),
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id"#
    )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(content)
        .bind(config)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Skill {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        content: row.try_get(4)?,
        config: row.try_get(5)?,
        created_by: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        plugin_installation_id: row.try_get(9)?,
    }))
}

pub async fn upsert_skill_file(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
    path: &str,
    content: &str,
) -> anyhow::Result<Option<SkillFile>> {
    let row = sqlx::query(
        r#"INSERT INTO skill_file (skill_id, path, content)
VALUES ($1, $2, $3)
ON CONFLICT (skill_id, path) DO UPDATE SET
    content = EXCLUDED.content,
    updated_at = now()
RETURNING id, skill_id, path, content, created_at, updated_at"#,
    )
    .bind(skill_id)
    .bind(path)
    .bind(content)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SkillFile {
        id: row.try_get(0)?,
        skill_id: row.try_get(1)?,
        path: row.try_get(2)?,
        content: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}
