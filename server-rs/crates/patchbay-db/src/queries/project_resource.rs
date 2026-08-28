//! Typed SQL queries for project_resource records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_project_resources(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(r#"SELECT count(*) FROM project_resource WHERE project_id = $1"#)
        .bind(project_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_project_resource(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_id: Uuid,
    workspace_id: Uuid,
    resource_type: &str,
    resource_ref: &serde_json::Value,
    label: Option<&str>,
    position: i32,
    created_by: Uuid,
) -> anyhow::Result<Option<ProjectResource>> {
    let row = sqlx::query(
        r#"INSERT INTO project_resource (
    project_id, workspace_id, resource_type, resource_ref, label, position, created_by
) VALUES (
    $1, $2, $3, $4, $5, $6, $7
) RETURNING id, project_id, workspace_id, resource_type, resource_ref, label, position, created_at, created_by"#
    )
        .bind(project_id)
        .bind(workspace_id)
        .bind(resource_type)
        .bind(resource_ref)
        .bind(label)
        .bind(position)
        .bind(created_by)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ProjectResource {
        id: row.try_get(0)?,
        project_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        resource_type: row.try_get(3)?,
        resource_ref: row.try_get(4)?,
        label: row.try_get(5)?,
        position: row.try_get(6)?,
        created_at: row.try_get(7)?,
        created_by: row.try_get(8)?,
    }))
}

pub async fn delete_project_resource(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM project_resource WHERE id = $1"#)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_project_resource(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<ProjectResource>> {
    let row = sqlx::query(
        r#"SELECT id, project_id, workspace_id, resource_type, resource_ref, label, position, created_at, created_by FROM project_resource
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ProjectResource {
        id: row.try_get(0)?,
        project_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        resource_type: row.try_get(3)?,
        resource_ref: row.try_get(4)?,
        label: row.try_get(5)?,
        position: row.try_get(6)?,
        created_at: row.try_get(7)?,
        created_by: row.try_get(8)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetProjectResourceCountsRow {
    pub project_id: Option<Uuid>,
    pub resource_count: i64,
}

pub async fn get_project_resource_counts(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<GetProjectResourceCountsRow>> {
    let rows = sqlx::query(
        r#"SELECT project_id, count(*)::bigint AS resource_count
FROM project_resource
WHERE project_id = ANY($1::uuid[])
GROUP BY project_id"#,
    )
    .bind(project_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetProjectResourceCountsRow {
            project_id: row.try_get(0)?,
            resource_count: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn get_project_resource_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<ProjectResource>> {
    let row = sqlx::query(
        r#"SELECT id, project_id, workspace_id, resource_type, resource_ref, label, position, created_at, created_by FROM project_resource
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ProjectResource {
        id: row.try_get(0)?,
        project_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        resource_type: row.try_get(3)?,
        resource_ref: row.try_get(4)?,
        label: row.try_get(5)?,
        position: row.try_get(6)?,
        created_at: row.try_get(7)?,
        created_by: row.try_get(8)?,
    }))
}

pub async fn list_project_resources(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ProjectResource>> {
    let rows = sqlx::query(
        r#"SELECT id, project_id, workspace_id, resource_type, resource_ref, label, position, created_at, created_by FROM project_resource
WHERE project_id = $1
ORDER BY position ASC, created_at ASC"#
    )
        .bind(project_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ProjectResource {
            id: row.try_get(0)?,
            project_id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            resource_type: row.try_get(3)?,
            resource_ref: row.try_get(4)?,
            label: row.try_get(5)?,
            position: row.try_get(6)?,
            created_at: row.try_get(7)?,
            created_by: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_project_resources_for_projects(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<ProjectResource>> {
    let rows = sqlx::query(
        r#"SELECT id, project_id, workspace_id, resource_type, resource_ref, label, position, created_at, created_by FROM project_resource
WHERE project_id = ANY($1::uuid[])
ORDER BY project_id, position ASC, created_at ASC"#
    )
        .bind(project_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ProjectResource {
            id: row.try_get(0)?,
            project_id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            resource_type: row.try_get(3)?,
            resource_ref: row.try_get(4)?,
            label: row.try_get(5)?,
            position: row.try_get(6)?,
            created_at: row.try_get(7)?,
            created_by: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn update_project_resource(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    resource_ref: &serde_json::Value,
    label: Option<&str>,
    position: i32,
) -> anyhow::Result<Option<ProjectResource>> {
    let row = sqlx::query(
        r#"UPDATE project_resource
SET resource_ref = $2,
    label        = $3,
    position     = $4
WHERE id = $1
RETURNING id, project_id, workspace_id, resource_type, resource_ref, label, position, created_at, created_by"#
    )
        .bind(id)
        .bind(resource_ref)
        .bind(label)
        .bind(position)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ProjectResource {
        id: row.try_get(0)?,
        project_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        resource_type: row.try_get(3)?,
        resource_ref: row.try_get(4)?,
        label: row.try_get(5)?,
        position: row.try_get(6)?,
        created_at: row.try_get(7)?,
        created_by: row.try_get(8)?,
    }))
}
