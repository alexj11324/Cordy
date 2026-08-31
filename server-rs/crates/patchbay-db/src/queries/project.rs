//! Typed SQL queries for project records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_issues_by_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM issue
WHERE project_id = $1"#,
    )
    .bind(project_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    title: &str,
    description: Option<&str>,
    icon: Option<&str>,
    status: &str,
    lead_type: Option<&str>,
    lead_id: Option<Uuid>,
    priority: &str,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
) -> anyhow::Result<Option<Project>> {
    let row = sqlx::query(
        r#"INSERT INTO project (
    workspace_id, title, description, icon, status,
    lead_type, lead_id, priority, start_date, due_date
) VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
) RETURNING id, workspace_id, title, description, icon, status, lead_type, lead_id, created_at, updated_at, priority, start_date, due_date"#
    )
        .bind(workspace_id)
        .bind(title)
        .bind(description)
        .bind(icon)
        .bind(status)
        .bind(lead_type)
        .bind(lead_id)
        .bind(priority)
        .bind(start_date)
        .bind(due_date)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Project {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        icon: row.try_get(4)?,
        status: row.try_get(5)?,
        lead_type: row.try_get(6)?,
        lead_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        priority: row.try_get(10)?,
        start_date: row.try_get(11)?,
        due_date: row.try_get(12)?,
    }))
}

pub async fn delete_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH deleted_linear_bindings AS (
               DELETE FROM linear_project_binding
               WHERE patchbay_project_id = $1 AND workspace_id = $2
           )
           DELETE FROM project WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Project>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, icon, status, lead_type, lead_id, created_at, updated_at, priority, start_date, due_date FROM project
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Project {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        icon: row.try_get(4)?,
        status: row.try_get(5)?,
        lead_type: row.try_get(6)?,
        lead_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        priority: row.try_get(10)?,
        start_date: row.try_get(11)?,
        due_date: row.try_get(12)?,
    }))
}

pub async fn get_project_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Project>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, title, description, icon, status, lead_type, lead_id, created_at, updated_at, priority, start_date, due_date FROM project
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Project {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        icon: row.try_get(4)?,
        status: row.try_get(5)?,
        lead_type: row.try_get(6)?,
        lead_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        priority: row.try_get(10)?,
        start_date: row.try_get(11)?,
        due_date: row.try_get(12)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetProjectIssueStatsRow {
    pub project_id: Option<Uuid>,
    pub total_count: i64,
    pub done_count: i64,
}

pub async fn get_project_issue_stats(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    project_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<GetProjectIssueStatsRow>> {
    let rows = sqlx::query(
        r#"SELECT project_id,
       count(*)::bigint AS total_count,
       count(*) FILTER (WHERE issue_effective_status(workspace_id, status) IN ('done', 'cancelled'))::bigint AS done_count
FROM issue
WHERE project_id = ANY($1::uuid[])
GROUP BY project_id"#
    )
        .bind(project_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetProjectIssueStatsRow {
            project_id: row.try_get(0)?,
            total_count: row.try_get(1)?,
            done_count: row.try_get(2)?,
        });
    }
    Ok(out)
}

pub async fn list_projects(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    status: Option<&str>,
    priority: Option<&str>,
) -> anyhow::Result<Vec<Project>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, title, description, icon, status, lead_type, lead_id, created_at, updated_at, priority, start_date, due_date FROM project
WHERE workspace_id = $1
  AND ($2::text IS NULL OR status = $2)
  AND ($3::text IS NULL OR priority = $3)
ORDER BY created_at DESC"#
    )
        .bind(workspace_id)
        .bind(status)
        .bind(priority)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Project {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            title: row.try_get(2)?,
            description: row.try_get(3)?,
            icon: row.try_get(4)?,
            status: row.try_get(5)?,
            lead_type: row.try_get(6)?,
            lead_id: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
            priority: row.try_get(10)?,
            start_date: row.try_get(11)?,
            due_date: row.try_get(12)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct SearchProjectsRow {
    pub project: Project,
    pub total_count: i64,
    pub match_source: String,
}

pub async fn search_projects(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    phrase: &str,
    terms: &[String],
    include_closed: bool,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<SearchProjectsRow>> {
    let rows = sqlx::query(
        r#"SELECT p.id, p.workspace_id, p.title, p.description, p.icon,
                  p.status, p.priority, p.lead_type, p.lead_id,
                  p.start_date, p.due_date, p.created_at, p.updated_at,
                  COUNT(*) OVER() AS total_count,
                  CASE
                    WHEN LOWER(p.title) LIKE '%' || $2 || '%'
                      OR (cardinality($3::text[]) > 1 AND NOT EXISTS (
                            SELECT 1 FROM unnest($3::text[]) AS term
                            WHERE LOWER(p.title) NOT LIKE '%' || term || '%'
                          ))
                    THEN 'title'
                    ELSE 'description'
                  END AS match_source
           FROM project p
           WHERE p.workspace_id = $1
             AND (
               LOWER(p.title) LIKE '%' || $2 || '%'
               OR LOWER(COALESCE(p.description, '')) LIKE '%' || $2 || '%'
               OR (cardinality($3::text[]) > 1 AND NOT EXISTS (
                     SELECT 1 FROM unnest($3::text[]) AS term
                     WHERE LOWER(p.title) NOT LIKE '%' || term || '%'
                       AND LOWER(COALESCE(p.description, '')) NOT LIKE '%' || term || '%'
                   ))
             )
             AND ($4 OR p.status NOT IN ('completed', 'cancelled'))
           ORDER BY
             CASE WHEN p.status = 'cancelled' AND LOWER(p.title) <> $2 THEN 1 ELSE 0 END,
             CASE
               WHEN LOWER(p.title) = $2 THEN 0
               WHEN LOWER(p.title) LIKE $2 || '%' THEN 1
               WHEN LOWER(p.title) LIKE '%' || $2 || '%' THEN 2
               WHEN cardinality($3::text[]) > 1 AND NOT EXISTS (
                      SELECT 1 FROM unnest($3::text[]) AS term
                      WHERE LOWER(p.title) NOT LIKE '%' || term || '%'
                    ) THEN 3
               WHEN LOWER(COALESCE(p.description, '')) LIKE '%' || $2 || '%' THEN 4
               ELSE 5
             END,
             p.updated_at DESC
           LIMIT $5 OFFSET $6"#,
    )
    .bind(workspace_id)
    .bind(phrase)
    .bind(terms)
    .bind(include_closed)
    .bind(limit)
    .bind(offset)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(SearchProjectsRow {
            project: Project {
                id: row.try_get(0)?,
                workspace_id: row.try_get(1)?,
                title: row.try_get(2)?,
                description: row.try_get(3)?,
                icon: row.try_get(4)?,
                status: row.try_get(5)?,
                priority: row.try_get(6)?,
                lead_type: row.try_get(7)?,
                lead_id: row.try_get(8)?,
                start_date: row.try_get(9)?,
                due_date: row.try_get(10)?,
                created_at: row.try_get(11)?,
                updated_at: row.try_get(12)?,
            },
            total_count: row.try_get(13)?,
            match_source: row.try_get(14)?,
        });
    }
    Ok(out)
}

pub async fn lock_project_for_chat_session_create(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM project
WHERE id = $1 AND workspace_id = $2
FOR KEY SHARE"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_project_for_delete(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM project
WHERE id = $1 AND workspace_id = $2
FOR UPDATE"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn update_project(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
    icon: Option<&str>,
    status: Option<&str>,
    priority: Option<&str>,
    lead_type: Option<&str>,
    lead_id: Option<Uuid>,
    start_date: Option<chrono::NaiveDate>,
    due_date: Option<chrono::NaiveDate>,
) -> anyhow::Result<Option<Project>> {
    let row = sqlx::query(
        r#"UPDATE project SET
    title = COALESCE($3, title),
    description = $4,
    icon = $5,
    status = COALESCE($6, status),
    priority = COALESCE($7, priority),
    lead_type = $8,
    lead_id = $9,
    start_date = $10,
    due_date = $11,
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, title, description, icon, status, lead_type, lead_id, created_at, updated_at, priority, start_date, due_date"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(title)
        .bind(description)
        .bind(icon)
        .bind(status)
        .bind(priority)
        .bind(lead_type)
        .bind(lead_id)
        .bind(start_date)
        .bind(due_date)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Project {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        icon: row.try_get(4)?,
        status: row.try_get(5)?,
        lead_type: row.try_get(6)?,
        lead_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        priority: row.try_get(10)?,
        start_date: row.try_get(11)?,
        due_date: row.try_get(12)?,
    }))
}
