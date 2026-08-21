//! Port of server/pkg/db/queries/issue_label.sql (generated issue_label.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn attach_label_to_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO agent_to_label (agent_id, label_id)
SELECT $1::uuid, $2::uuid
WHERE EXISTS (
    SELECT 1 FROM agent a
    WHERE a.id = $1::uuid
      AND a.workspace_id = $3::uuid
)
AND EXISTS (
    SELECT 1 FROM issue_label l
    WHERE l.id = $2::uuid
      AND l.workspace_id = $3::uuid
      AND l.resource_type = 'agent'
)
ON CONFLICT DO NOTHING"#,
    )
    .bind(agent_id)
    .bind(label_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AttachLabelToIssueRow {
    pub changed: bool,
    pub issue_revision: i64,
}

pub async fn attach_label_to_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<AttachLabelToIssueRow>> {
    let row = sqlx::query(
        r#"WITH inserted AS (
    INSERT INTO issue_to_label (issue_id, label_id)
    SELECT $1::uuid, $2::uuid
    WHERE EXISTS (
        SELECT 1 FROM issue i
        WHERE i.id = $1::uuid
          AND i.workspace_id = $3::uuid
    )
      AND EXISTS (
        SELECT 1 FROM issue_label l
        WHERE l.id = $2::uuid
          AND l.workspace_id = $3::uuid
          AND l.resource_type = 'issue'
    )
    ON CONFLICT DO NOTHING
    RETURNING issue_id
), bumped AS (
    UPDATE issue
    SET revision = revision + 1,
        last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now())
    WHERE id IN (SELECT issue_id FROM inserted)
    RETURNING revision
)
SELECT EXISTS(SELECT 1 FROM inserted) AS changed,
       COALESCE((SELECT revision FROM bumped), 0)::bigint AS issue_revision"#,
    )
    .bind(issue_id)
    .bind(label_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AttachLabelToIssueRow {
        changed: row.try_get(0)?,
        issue_revision: row.try_get(1)?,
    }))
}

pub async fn attach_label_to_issue_on_create(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH touched_issue AS (
    UPDATE issue
    SET last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now())
    WHERE issue.id = $1::uuid
      AND issue.workspace_id = $3::uuid
      AND NOT EXISTS (
          SELECT 1 FROM issue_to_label
          WHERE issue_to_label.issue_id = $1::uuid
            AND issue_to_label.label_id = $2::uuid
      )
      AND EXISTS (
          SELECT 1 FROM issue_label
          WHERE issue_label.id = $2::uuid
            AND issue_label.workspace_id = $3::uuid
            AND issue_label.resource_type = 'issue'
      )
    RETURNING issue.id
)
INSERT INTO issue_to_label (issue_id, label_id)
SELECT $1::uuid, $2::uuid
WHERE EXISTS (SELECT 1 FROM touched_issue)
ON CONFLICT DO NOTHING"#,
    )
    .bind(issue_id)
    .bind(label_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn attach_label_to_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO skill_to_label (skill_id, label_id)
SELECT $1::uuid, $2::uuid
WHERE EXISTS (
    SELECT 1 FROM skill s
    WHERE s.id = $1::uuid
      AND s.workspace_id = $3::uuid
)
AND EXISTS (
    SELECT 1 FROM issue_label l
    WHERE l.id = $2::uuid
      AND l.workspace_id = $3::uuid
      AND l.resource_type = 'skill'
)
ON CONFLICT DO NOTHING"#,
    )
    .bind(skill_id)
    .bind(label_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn create_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    resource_type: &str,
    name: &str,
    description: &str,
    color: &str,
) -> anyhow::Result<Option<IssueLabel>> {
    let row = sqlx::query(
        r#"INSERT INTO issue_label (workspace_id, resource_type, name, description, color)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, workspace_id, name, color, created_at, updated_at, resource_type, description"#,
    )
    .bind(workspace_id)
    .bind(resource_type)
    .bind(name)
    .bind(description)
    .bind(color)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueLabel {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        color: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        resource_type: row.try_get(6)?,
        description: row.try_get(7)?,
    }))
}

pub async fn delete_agent_label_assignments_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent_to_label WHERE agent_id = $1"#)
        .bind(agent_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_agent_label_assignments_by_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    label_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent_to_label WHERE label_id = $1"#)
        .bind(label_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_agent_label_assignments_by_system_runtime_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_to_label
WHERE agent_id IN (SELECT id FROM agent WHERE runtime_id = $1 AND kind = 'system')"#,
    )
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_issue_label_assignments_by_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    label_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM issue_to_label WHERE label_id = $1"#)
        .bind(label_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"DELETE FROM issue_label
WHERE id = $1 AND workspace_id = $2
RETURNING id"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn delete_skill_label_assignments_by_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    label_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM skill_to_label WHERE label_id = $1"#)
        .bind(label_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_skill_label_assignments_by_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM skill_to_label WHERE skill_id = $1"#)
        .bind(skill_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn detach_label_from_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_to_label
WHERE agent_id = $1::uuid
  AND label_id = $2::uuid
  AND EXISTS (
      SELECT 1 FROM agent a
      WHERE a.id = $1::uuid
        AND a.workspace_id = $3::uuid
  )"#,
    )
    .bind(agent_id)
    .bind(label_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DetachLabelFromIssueRow {
    pub changed: bool,
    pub issue_revision: i64,
}

pub async fn detach_label_from_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<DetachLabelFromIssueRow>> {
    let row = sqlx::query(
        r#"WITH deleted AS (
    DELETE FROM issue_to_label
    WHERE issue_id = $1::uuid
      AND label_id = $2::uuid
      AND EXISTS (
          SELECT 1 FROM issue i
          WHERE i.id = $1::uuid
            AND i.workspace_id = $3::uuid
      )
    RETURNING issue_id
), bumped AS (
    UPDATE issue
    SET revision = revision + 1,
        last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now())
    WHERE id IN (SELECT issue_id FROM deleted)
    RETURNING revision
)
SELECT EXISTS(SELECT 1 FROM deleted) AS changed,
       COALESCE((SELECT revision FROM bumped), 0)::bigint AS issue_revision"#,
    )
    .bind(issue_id)
    .bind(label_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DetachLabelFromIssueRow {
        changed: row.try_get(0)?,
        issue_revision: row.try_get(1)?,
    }))
}

pub async fn detach_label_from_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
    label_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM skill_to_label
WHERE skill_id = $1::uuid
  AND label_id = $2::uuid
  AND EXISTS (
      SELECT 1 FROM skill s
      WHERE s.id = $1::uuid
        AND s.workspace_id = $3::uuid
  )"#,
    )
    .bind(skill_id)
    .bind(label_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<IssueLabel>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, color, created_at, updated_at, resource_type, description FROM issue_label
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueLabel {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        color: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        resource_type: row.try_get(6)?,
        description: row.try_get(7)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListLabelsRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub color: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub resource_type: String,
    pub description: String,
    pub usage_count: i64,
}

pub async fn list_labels(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    resource_type: &str,
) -> anyhow::Result<Vec<ListLabelsRow>> {
    let rows = sqlx::query(
        r#"SELECT l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description,
    CASE l.resource_type
        WHEN 'issue' THEN (SELECT COUNT(*) FROM issue_to_label x WHERE x.label_id = l.id)
        WHEN 'agent' THEN (SELECT COUNT(*) FROM agent_to_label x WHERE x.label_id = l.id)
        WHEN 'skill' THEN (SELECT COUNT(*) FROM skill_to_label x WHERE x.label_id = l.id)
        ELSE 0
    END::bigint AS usage_count
FROM issue_label l
WHERE l.workspace_id = $1::uuid
  AND l.resource_type = $2::text
ORDER BY LOWER(name) ASC"#
    )
        .bind(workspace_id)
        .bind(resource_type)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListLabelsRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            color: row.try_get(3)?,
            created_at: row.try_get(4)?,
            updated_at: row.try_get(5)?,
            resource_type: row.try_get(6)?,
            description: row.try_get(7)?,
            usage_count: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_labels_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<IssueLabel>> {
    let rows = sqlx::query(
        r#"SELECT l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description
FROM issue_label l
JOIN agent_to_label atl ON atl.label_id = l.id
WHERE atl.agent_id = $1::uuid
  AND l.workspace_id = $2::uuid
  AND l.resource_type = 'agent'
ORDER BY LOWER(l.name) ASC"#
    )
        .bind(agent_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueLabel {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            color: row.try_get(3)?,
            created_at: row.try_get(4)?,
            updated_at: row.try_get(5)?,
            resource_type: row.try_get(6)?,
            description: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn list_labels_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<IssueLabel>> {
    let rows = sqlx::query(
        r#"SELECT l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description
FROM issue_label l
JOIN issue_to_label il ON il.label_id = l.id
WHERE il.issue_id = $1::uuid
  AND l.workspace_id = $2::uuid
  AND l.resource_type = 'issue'
ORDER BY LOWER(l.name) ASC"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueLabel {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            color: row.try_get(3)?,
            created_at: row.try_get(4)?,
            updated_at: row.try_get(5)?,
            resource_type: row.try_get(6)?,
            description: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn list_labels_by_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<IssueLabel>> {
    let rows = sqlx::query(
        r#"SELECT l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description
FROM issue_label l
JOIN skill_to_label stl ON stl.label_id = l.id
WHERE stl.skill_id = $1::uuid
  AND l.workspace_id = $2::uuid
  AND l.resource_type = 'skill'
ORDER BY LOWER(l.name) ASC"#
    )
        .bind(skill_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueLabel {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            color: row.try_get(3)?,
            created_at: row.try_get(4)?,
            updated_at: row.try_get(5)?,
            resource_type: row.try_get(6)?,
            description: row.try_get(7)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListLabelsForAgentsRow {
    pub agent_id: Option<Uuid>,
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub color: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub resource_type: String,
    pub description: String,
}

pub async fn list_labels_for_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_ids: Vec<Uuid>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListLabelsForAgentsRow>> {
    let rows = sqlx::query(
        r#"SELECT atl.agent_id, l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description
FROM issue_label l
JOIN agent_to_label atl ON atl.label_id = l.id
WHERE atl.agent_id = ANY($1::uuid[])
  AND l.workspace_id = $2::uuid
  AND l.resource_type = 'agent'
ORDER BY atl.agent_id, LOWER(l.name) ASC"#
    )
        .bind(agent_ids)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListLabelsForAgentsRow {
            agent_id: row.try_get(0)?,
            id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            name: row.try_get(3)?,
            color: row.try_get(4)?,
            created_at: row.try_get(5)?,
            updated_at: row.try_get(6)?,
            resource_type: row.try_get(7)?,
            description: row.try_get(8)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListLabelsForIssuesRow {
    pub issue_id: Option<Uuid>,
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub color: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub resource_type: String,
    pub description: String,
}

pub async fn list_labels_for_issues(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_ids: Vec<Uuid>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListLabelsForIssuesRow>> {
    let rows = sqlx::query(
        r#"SELECT il.issue_id, l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description
FROM issue_label l
JOIN issue_to_label il ON il.label_id = l.id
WHERE il.issue_id = ANY($1::uuid[])
  AND l.workspace_id = $2::uuid
  AND l.resource_type = 'issue'
ORDER BY il.issue_id, LOWER(l.name) ASC"#
    )
        .bind(issue_ids)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListLabelsForIssuesRow {
            issue_id: row.try_get(0)?,
            id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            name: row.try_get(3)?,
            color: row.try_get(4)?,
            created_at: row.try_get(5)?,
            updated_at: row.try_get(6)?,
            resource_type: row.try_get(7)?,
            description: row.try_get(8)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListLabelsForSkillsRow {
    pub skill_id: Option<Uuid>,
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub color: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub resource_type: String,
    pub description: String,
}

pub async fn list_labels_for_skills(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    skill_ids: Vec<Uuid>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListLabelsForSkillsRow>> {
    let rows = sqlx::query(
        r#"SELECT stl.skill_id, l.id, l.workspace_id, l.name, l.color, l.created_at, l.updated_at, l.resource_type, l.description
FROM issue_label l
JOIN skill_to_label stl ON stl.label_id = l.id
WHERE stl.skill_id = ANY($1::uuid[])
  AND l.workspace_id = $2::uuid
  AND l.resource_type = 'skill'
ORDER BY stl.skill_id, LOWER(l.name) ASC"#
    )
        .bind(skill_ids)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListLabelsForSkillsRow {
            skill_id: row.try_get(0)?,
            id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            name: row.try_get(3)?,
            color: row.try_get(4)?,
            created_at: row.try_get(5)?,
            updated_at: row.try_get(6)?,
            resource_type: row.try_get(7)?,
            description: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn update_label(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    color: Option<&str>,
) -> anyhow::Result<Option<IssueLabel>> {
    let row = sqlx::query(
        r#"UPDATE issue_label SET
    name = COALESCE($3, name),
    description = COALESCE($4, description),
    color = COALESCE($5, color),
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, name, color, created_at, updated_at, resource_type, description"#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(name)
    .bind(description)
    .bind(color)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueLabel {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        color: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        resource_type: row.try_get(6)?,
        description: row.try_get(7)?,
    }))
}
