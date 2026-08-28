//! Typed SQL queries for runtime_profile records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_agents_by_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    profile_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM agent a
JOIN agent_runtime ar ON ar.id = a.runtime_id
WHERE ar.profile_id = $1 AND ar.workspace_id = $2 AND a.archived_at IS NULL"#,
    )
    .bind(profile_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_runtime_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    display_name: &str,
    protocol_family: &str,
    command_name: &str,
    description: Option<&str>,
    fixed_args: &serde_json::Value,
    visibility: &str,
    created_by: Uuid,
    enabled: bool,
) -> anyhow::Result<Option<RuntimeProfile>> {
    let row = sqlx::query(
        r#"INSERT INTO runtime_profile (
    workspace_id,
    display_name,
    protocol_family,
    command_name,
    description,
    fixed_args,
    visibility,
    created_by,
    enabled
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
RETURNING id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(display_name)
        .bind(protocol_family)
        .bind(command_name)
        .bind(description)
        .bind(fixed_args)
        .bind(visibility)
        .bind(created_by)
        .bind(enabled)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RuntimeProfile {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        protocol_family: row.try_get(3)?,
        command_name: row.try_get(4)?,
        description: row.try_get(5)?,
        fixed_args: row.try_get(6)?,
        visibility: row.try_get(7)?,
        created_by: row.try_get(8)?,
        enabled: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteAgentRuntimesByProfileRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub daemon_id: Option<String>,
    pub provider: String,
}

pub async fn delete_agent_runtimes_by_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    profile_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<DeleteAgentRuntimesByProfileRow>> {
    let rows = sqlx::query(
        r#"DELETE FROM agent_runtime
WHERE profile_id = $1 AND workspace_id = $2
RETURNING id, workspace_id, owner_id, daemon_id, provider"#,
    )
    .bind(profile_id)
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(DeleteAgentRuntimesByProfileRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            owner_id: row.try_get(2)?,
            daemon_id: row.try_get(3)?,
            provider: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn delete_runtime_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM runtime_profile
WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_runtime_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<RuntimeProfile>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at FROM runtime_profile
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RuntimeProfile {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        protocol_family: row.try_get(3)?,
        command_name: row.try_get(4)?,
        description: row.try_get(5)?,
        fixed_args: row.try_get(6)?,
        visibility: row.try_get(7)?,
        created_by: row.try_get(8)?,
        enabled: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn get_runtime_profile_for_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<RuntimeProfile>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at FROM runtime_profile
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RuntimeProfile {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        protocol_family: row.try_get(3)?,
        command_name: row.try_get(4)?,
        description: row.try_get(5)?,
        fixed_args: row.try_get(6)?,
        visibility: row.try_get(7)?,
        created_by: row.try_get(8)?,
        enabled: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn list_agent_runtime_i_ds_by_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    profile_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM agent_runtime
WHERE profile_id = $1 AND workspace_id = $2
ORDER BY id
FOR UPDATE"#,
    )
    .bind(profile_id)
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_enabled_runtime_profiles_for_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<RuntimeProfile>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at FROM runtime_profile
WHERE workspace_id = $1 AND enabled = true
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(RuntimeProfile {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            display_name: row.try_get(2)?,
            protocol_family: row.try_get(3)?,
            command_name: row.try_get(4)?,
            description: row.try_get(5)?,
            fixed_args: row.try_get(6)?,
            visibility: row.try_get(7)?,
            created_by: row.try_get(8)?,
            enabled: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_runtime_profiles(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<RuntimeProfile>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at FROM runtime_profile
WHERE workspace_id = $1
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(RuntimeProfile {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            display_name: row.try_get(2)?,
            protocol_family: row.try_get(3)?,
            command_name: row.try_get(4)?,
            description: row.try_get(5)?,
            fixed_args: row.try_get(6)?,
            visibility: row.try_get(7)?,
            created_by: row.try_get(8)?,
            enabled: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn lock_runtime_profile_for_delete(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<RuntimeProfile>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at FROM runtime_profile
WHERE id = $1 AND workspace_id = $2
FOR UPDATE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RuntimeProfile {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        protocol_family: row.try_get(3)?,
        command_name: row.try_get(4)?,
        description: row.try_get(5)?,
        fixed_args: row.try_get(6)?,
        visibility: row.try_get(7)?,
        created_by: row.try_get(8)?,
        enabled: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn lock_runtime_profile_for_registration(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<RuntimeProfile>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at FROM runtime_profile
WHERE id = $1 AND workspace_id = $2
FOR KEY SHARE"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RuntimeProfile {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        protocol_family: row.try_get(3)?,
        command_name: row.try_get(4)?,
        description: row.try_get(5)?,
        fixed_args: row.try_get(6)?,
        visibility: row.try_get(7)?,
        created_by: row.try_get(8)?,
        enabled: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn update_runtime_profile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    display_name: Option<&str>,
    command_name: Option<&str>,
    description: Option<&str>,
    fixed_args: &serde_json::Value,
    visibility: Option<&str>,
    enabled: Option<bool>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<RuntimeProfile>> {
    let row = sqlx::query(
        r#"UPDATE runtime_profile
SET display_name = COALESCE($1, display_name),
    command_name = COALESCE($2, command_name),
    description  = COALESCE($3, description),
    fixed_args   = COALESCE($4, fixed_args),
    visibility   = COALESCE($5, visibility),
    enabled      = COALESCE($6, enabled),
    updated_at   = now()
WHERE id = $7 AND workspace_id = $8
RETURNING id, workspace_id, display_name, protocol_family, command_name, description, fixed_args, visibility, created_by, enabled, created_at, updated_at"#
    )
        .bind(display_name)
        .bind(command_name)
        .bind(description)
        .bind(fixed_args)
        .bind(visibility)
        .bind(enabled)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RuntimeProfile {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        display_name: row.try_get(2)?,
        protocol_family: row.try_get(3)?,
        command_name: row.try_get(4)?,
        description: row.try_get(5)?,
        fixed_args: row.try_get(6)?,
        visibility: row.try_get(7)?,
        created_by: row.try_get(8)?,
        enabled: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}
