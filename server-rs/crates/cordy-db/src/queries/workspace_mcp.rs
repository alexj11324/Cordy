//! Typed SQL queries for workspace_mcp records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn add_agent_mcp_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    server_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO agent_mcp_server (agent_id, server_id)
VALUES ($1, $2)
ON CONFLICT DO NOTHING"#,
    )
    .bind(agent_id)
    .bind(server_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn create_workspace_mcp_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    config: &serde_json::Value,
    created_by: Uuid,
) -> anyhow::Result<Option<WorkspaceMcpServer>> {
    let row = sqlx::query(
        r#"INSERT INTO workspace_mcp_server (workspace_id, name, config, created_by)
VALUES ($1, $2, $3, $4)
RETURNING id, workspace_id, name, config, created_by, created_at, updated_at"#,
    )
    .bind(workspace_id)
    .bind(name)
    .bind(config)
    .bind(created_by)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceMcpServer {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        config: row.try_get(3)?,
        created_by: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
    }))
}

pub async fn delete_agent_mcp_servers_by_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    server_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent_mcp_server WHERE server_id = $1"#)
        .bind(server_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_workspace_mcp_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM workspace_mcp_server
WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_workspace_mcp_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<WorkspaceMcpServer>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, config, created_by, created_at, updated_at FROM workspace_mcp_server
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceMcpServer {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        config: row.try_get(3)?,
        created_by: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListAgentMcpServersRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub config: Option<serde_json::Value>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub enabled: bool,
}

pub async fn list_agent_mcp_servers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<ListAgentMcpServersRow>> {
    let rows = sqlx::query(
        r#"SELECT s.id, s.workspace_id, s.name, s.config, s.created_at, s.updated_at, ams.enabled
FROM workspace_mcp_server s
JOIN agent_mcp_server ams ON ams.server_id = s.id
WHERE ams.agent_id = $1
ORDER BY s.name ASC"#,
    )
    .bind(agent_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListAgentMcpServersRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            config: row.try_get(3)?,
            created_at: row.try_get(4)?,
            updated_at: row.try_get(5)?,
            enabled: row.try_get(6)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListEnabledAgentMcpServersRow {
    pub name: String,
    pub config: Option<serde_json::Value>,
}

pub async fn list_enabled_agent_mcp_servers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
) -> anyhow::Result<Vec<ListEnabledAgentMcpServersRow>> {
    let rows = sqlx::query(
        r#"SELECT s.name, s.config
FROM workspace_mcp_server s
JOIN agent_mcp_server ams ON ams.server_id = s.id
WHERE ams.agent_id = $1 AND ams.enabled = TRUE
ORDER BY s.name ASC"#,
    )
    .bind(agent_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListEnabledAgentMcpServersRow {
            name: row.try_get(0)?,
            config: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn list_workspace_mcp_servers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<WorkspaceMcpServer>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, config, created_by, created_at, updated_at FROM workspace_mcp_server
WHERE workspace_id = $1
ORDER BY name ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(WorkspaceMcpServer {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            config: row.try_get(3)?,
            created_by: row.try_get(4)?,
            created_at: row.try_get(5)?,
            updated_at: row.try_get(6)?,
        });
    }
    Ok(out)
}

pub async fn lock_workspace_mcp_server_for_share(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM workspace_mcp_server
WHERE id = $1 AND workspace_id = $2
FOR SHARE"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_workspace_mcp_server_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM workspace_mcp_server
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

pub async fn remove_agent_mcp_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    server_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_mcp_server
WHERE agent_id = $1 AND server_id = $2"#,
    )
    .bind(agent_id)
    .bind(server_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_agent_mcp_server_enabled(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    agent_id: Uuid,
    server_id: Uuid,
    enabled: bool,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE agent_mcp_server
SET enabled = $3
WHERE agent_id = $1 AND server_id = $2"#,
    )
    .bind(agent_id)
    .bind(server_id)
    .bind(enabled)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_workspace_mcp_server(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    name: Option<&str>,
    config: Option<&serde_json::Value>,
) -> anyhow::Result<Option<WorkspaceMcpServer>> {
    let row = sqlx::query(
        r#"UPDATE workspace_mcp_server SET
    name = COALESCE($3, name),
    config = COALESCE($4, config),
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, name, config, created_by, created_at, updated_at"#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(name)
    .bind(config)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceMcpServer {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        config: row.try_get(3)?,
        created_by: row.try_get(4)?,
        created_at: row.try_get(5)?,
        updated_at: row.try_get(6)?,
    }))
}
