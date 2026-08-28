//! Typed SQL queries for issue_property records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_active_issue_properties(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT COUNT(*) FROM issue_property
WHERE workspace_id = $1 AND archived_at IS NULL"#,
    )
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CountIssuesUsingPropertyOptionsRow {
    pub option_id: String,
    pub usage_count: i64,
}

pub async fn count_issues_using_property_options(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    option_ids: &[String],
    workspace_id: Uuid,
    property_key: &str,
) -> anyhow::Result<Vec<CountIssuesUsingPropertyOptionsRow>> {
    let rows = sqlx::query(
        r#"SELECT opt::text AS option_id, COUNT(i.id) AS usage_count
FROM unnest($1::text[]) AS opt
LEFT JOIN issue i
  ON i.workspace_id = $2::uuid
 AND (i.properties -> $3::text) ? opt
GROUP BY opt
HAVING COUNT(i.id) > 0"#,
    )
    .bind(option_ids)
    .bind(workspace_id)
    .bind(property_key)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(CountIssuesUsingPropertyOptionsRow {
            option_id: row.try_get(0)?,
            usage_count: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn create_issue_property(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    type_: &str,
    description: &str,
    icon: &str,
    config: &serde_json::Value,
) -> anyhow::Result<Option<IssueProperty>> {
    let row = sqlx::query(
        r#"INSERT INTO issue_property (workspace_id, name, type, description, icon, config, position)
SELECT $1::uuid,
       $2::text,
       $3::text,
       $4::text,
       $5::text,
       $6::jsonb,
       COALESCE((SELECT MAX(position) FROM issue_property WHERE workspace_id = $1::uuid), 0) + 1
RETURNING id, workspace_id, name, type, description, config, position, archived_at, created_at, updated_at, icon"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(type_)
        .bind(description)
        .bind(icon)
        .bind(config)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueProperty {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        type_: row.try_get(3)?,
        description: row.try_get(4)?,
        config: row.try_get(5)?,
        position: row.try_get(6)?,
        archived_at: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        icon: row.try_get(10)?,
    }))
}

pub async fn delete_issue_property_value(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    key: &str,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"UPDATE issue
SET properties = properties - $1::text,
    revision = revision + CASE WHEN properties ? $1::text THEN 1 ELSE 0 END,
    last_activity_at = CASE
        WHEN properties ? $1::text
        THEN GREATEST(COALESCE(last_activity_at, updated_at), now())
        ELSE last_activity_at
    END,
    updated_at = CASE WHEN properties ? $1::text THEN now() ELSE updated_at END
WHERE id = $2::uuid AND workspace_id = $3::uuid
RETURNING id, workspace_id, title, description, status, priority, assignee_type, assignee_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at"#
    )
        .bind(key)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        assignee_type: row.try_get(6)?,
        assignee_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
    }))
}

pub async fn get_issue_property(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<IssueProperty>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, type, description, config, position, archived_at, created_at, updated_at, icon FROM issue_property
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueProperty {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        type_: row.try_get(3)?,
        description: row.try_get(4)?,
        config: row.try_get(5)?,
        position: row.try_get(6)?,
        archived_at: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        icon: row.try_get(10)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListIssuePropertiesRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub description: String,
    pub config: Option<serde_json::Value>,
    pub position: f64,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub icon: String,
    pub usage_count: i64,
}

pub async fn list_issue_properties(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    include_archived: bool,
) -> anyhow::Result<Vec<ListIssuePropertiesRow>> {
    let rows = sqlx::query(
        r#"SELECT p.id, p.workspace_id, p.name, p.type, p.description, p.config, p.position, p.archived_at, p.created_at, p.updated_at, p.icon,
    (
        SELECT COUNT(*) FROM issue i
        WHERE i.workspace_id = p.workspace_id
          AND i.properties ? p.id::text
    )::bigint AS usage_count
FROM issue_property p
WHERE p.workspace_id = $1::uuid
  AND ($2::bool OR p.archived_at IS NULL)
ORDER BY p.position ASC, LOWER(p.name) ASC"#
    )
        .bind(workspace_id)
        .bind(include_archived)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListIssuePropertiesRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            type_: row.try_get(3)?,
            description: row.try_get(4)?,
            config: row.try_get(5)?,
            position: row.try_get(6)?,
            archived_at: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
            icon: row.try_get(10)?,
            usage_count: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn set_issue_property_value(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    key: &str,
    value: &serde_json::Value,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Issue>> {
    let row = sqlx::query(
        r#"UPDATE issue
SET properties = jsonb_set(properties, ARRAY[$1::text], $2::jsonb, true),
    revision = revision + CASE WHEN properties -> $1::text IS DISTINCT FROM $2::jsonb THEN 1 ELSE 0 END,
    last_activity_at = CASE
        WHEN properties -> $1::text IS DISTINCT FROM $2::jsonb
        THEN GREATEST(COALESCE(last_activity_at, updated_at), now())
        ELSE last_activity_at
    END,
    updated_at = CASE WHEN properties -> $1::text IS DISTINCT FROM $2::jsonb THEN now() ELSE updated_at END
WHERE id = $3::uuid AND workspace_id = $4::uuid
RETURNING id, workspace_id, title, description, status, priority, assignee_type, assignee_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at"#
    )
        .bind(key)
        .bind(value)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Issue {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        status: row.try_get(4)?,
        priority: row.try_get(5)?,
        assignee_type: row.try_get(6)?,
        assignee_id: row.try_get(7)?,
        creator_type: row.try_get(8)?,
        creator_id: row.try_get(9)?,
        parent_issue_id: row.try_get(10)?,
        acceptance_criteria: row.try_get(11)?,
        context_refs: row.try_get(12)?,
        position: row.try_get(13)?,
        due_date: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
        number: row.try_get(17)?,
        project_id: row.try_get(18)?,
        origin_type: row.try_get(19)?,
        origin_id: row.try_get(20)?,
        first_executed_at: row.try_get(21)?,
        start_date: row.try_get(22)?,
        metadata: row.try_get(23)?,
        stage: row.try_get(24)?,
        properties: row.try_get(25)?,
        revision: row.try_get(26)?,
        last_activity_at: row.try_get(27)?,
    }))
}

pub async fn update_issue_property(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    config: Option<&serde_json::Value>,
    archived_set: bool,
    archived_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<IssueProperty>> {
    let row = sqlx::query(
        r#"UPDATE issue_property SET
    name = COALESCE($3, name),
    description = COALESCE($4, description),
    icon = COALESCE($5, icon),
    config = COALESCE($6, config),
    archived_at = CASE WHEN $7::bool THEN $8 ELSE archived_at END,
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, name, type, description, config, position, archived_at, created_at, updated_at, icon"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(icon)
        .bind(config)
        .bind(archived_set)
        .bind(archived_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueProperty {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        type_: row.try_get(3)?,
        description: row.try_get(4)?,
        config: row.try_get(5)?,
        position: row.try_get(6)?,
        archived_at: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        icon: row.try_get(10)?,
    }))
}
