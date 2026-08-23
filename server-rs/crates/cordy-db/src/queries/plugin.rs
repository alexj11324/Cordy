//! Port of server/pkg/db/queries/plugin.sql (generated plugin.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_recent_plugin_failures(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    hook_key: &str,
    created_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM plugin_invocation
WHERE installation_id = $1 AND hook_key = $2 AND created_at > $3 AND status <> 'ok'"#,
    )
    .bind(installation_id)
    .bind(hook_key)
    .bind(created_at)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn count_recent_plugin_invocations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    hook_key: &str,
    created_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM plugin_invocation
WHERE installation_id = $1 AND hook_key = $2 AND created_at > $3"#,
    )
    .bind(installation_id)
    .bind(hook_key)
    .bind(created_at)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_plugin_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    plugin_key: &str,
    source_url: &str,
    version: &str,
    manifest: &serde_json::Value,
    granted_scopes: &serde_json::Value,
    installed_by: Uuid,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"INSERT INTO plugin_installation (
    workspace_id, plugin_key, source_url, version, manifest, granted_scopes, installed_by
) VALUES ($1, $2, $3, $4, $5, $6, $7)
RETURNING id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals"#
    )
        .bind(workspace_id)
        .bind(plugin_key)
        .bind(source_url)
        .bind(version)
        .bind(manifest)
        .bind(granted_scopes)
        .bind(installed_by)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn create_plugin_invocation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    workspace_id: Uuid,
    hook_key: &str,
    trigger: &str,
    status: &str,
    attempt: i32,
    latency_ms: i32,
    event_type: Option<&str>,
    error: Option<&str>,
) -> anyhow::Result<Option<PluginInvocation>> {
    let row = sqlx::query(
        r#"INSERT INTO plugin_invocation (
    installation_id, workspace_id, hook_key, trigger, status, event_type, attempt, latency_ms, error
) VALUES ($1, $2, $3, $4, $5, $8, $6, $7, $9)
RETURNING id, installation_id, workspace_id, hook_key, trigger, status, event_type, attempt, latency_ms, error, created_at"#
    )
        .bind(installation_id)
        .bind(workspace_id)
        .bind(hook_key)
        .bind(trigger)
        .bind(status)
        .bind(attempt)
        .bind(latency_ms)
        .bind(event_type)
        .bind(error)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInvocation {
        id: row.try_get(0)?,
        installation_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        hook_key: row.try_get(3)?,
        trigger: row.try_get(4)?,
        status: row.try_get(5)?,
        event_type: row.try_get(6)?,
        attempt: row.try_get(7)?,
        latency_ms: row.try_get(8)?,
        error: row.try_get(9)?,
        created_at: row.try_get(10)?,
    }))
}

pub async fn delete_expired_plugin_invocations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    created_at: Option<DateTime<Utc>>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM plugin_invocation WHERE created_at < $1"#)
        .bind(created_at)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM plugin_installation WHERE id = $1"#)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_invocations_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM plugin_invocation WHERE installation_id = $1"#)
        .bind(installation_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_secret(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    key: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM plugin_secret WHERE installation_id = $1 AND key = $2"#)
        .bind(installation_id)
        .bind(key)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_secrets_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM plugin_secret WHERE installation_id = $1"#)
        .bind(installation_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_skills_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    plugin_installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM skill WHERE plugin_installation_id = $1"#)
        .bind(plugin_installation_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_skills_not_in(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    plugin_installation_id: Uuid,
    keep_names: &[String],
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM skill
WHERE plugin_installation_id = $1 AND name <> ALL($2::text[])"#,
    )
    .bind(plugin_installation_id)
    .bind(keep_names)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_storage_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM plugin_storage WHERE installation_id = $1"#)
        .bind(installation_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_plugin_storage_value(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM plugin_storage
WHERE installation_id = $1 AND scope_type = $2 AND scope_id = $3 AND key = $4"#,
    )
    .bind(installation_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(key)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_plugin_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals FROM plugin_installation WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn get_plugin_installation_by_token_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: Option<&str>,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals FROM plugin_installation WHERE token_hash = $1"#
    )
        .bind(token_hash)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn get_plugin_secret(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<PluginSecret>> {
    let row = sqlx::query(
        r#"SELECT id, installation_id, key, ciphertext, created_at, updated_at FROM plugin_secret
WHERE installation_id = $1 AND key = $2"#,
    )
    .bind(installation_id)
    .bind(key)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginSecret {
        id: row.try_get(0)?,
        installation_id: row.try_get(1)?,
        key: row.try_get(2)?,
        ciphertext: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetPluginStorageUsageRow {
    pub key_count: i64,
    pub total_bytes: i64,
}

pub async fn get_plugin_storage_usage(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<GetPluginStorageUsageRow>> {
    let row = sqlx::query(
        r#"SELECT COUNT(*)::bigint AS key_count,
       COALESCE(SUM(octet_length(value)), 0)::bigint AS total_bytes
FROM plugin_storage
WHERE installation_id = $1 AND scope_type = $2 AND scope_id = $3 AND key <> $4"#,
    )
    .bind(installation_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(key)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetPluginStorageUsageRow {
        key_count: row.try_get(0)?,
        total_bytes: row.try_get(1)?,
    }))
}

pub async fn get_plugin_storage_value(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<PluginStorage>> {
    let row = sqlx::query(
        r#"SELECT id, installation_id, scope_type, scope_id, key, value, created_at, updated_at FROM plugin_storage
WHERE installation_id = $1 AND scope_type = $2 AND scope_id = $3 AND key = $4"#
    )
        .bind(installation_id)
        .bind(scope_type)
        .bind(scope_id)
        .bind(key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginStorage {
        id: row.try_get(0)?,
        installation_id: row.try_get(1)?,
        scope_type: row.try_get(2)?,
        scope_id: row.try_get(3)?,
        key: row.try_get(4)?,
        value: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}

pub async fn get_workspace_plugin_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals FROM plugin_installation
WHERE workspace_id = $1 AND id = $2"#
    )
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn get_workspace_plugin_installation_by_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    plugin_key: &str,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals FROM plugin_installation
WHERE workspace_id = $1 AND plugin_key = $2"#
    )
        .bind(workspace_id)
        .bind(plugin_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn list_plugin_invocations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<PluginInvocation>> {
    let rows = sqlx::query(
        r#"SELECT id, installation_id, workspace_id, hook_key, trigger, status, event_type, attempt, latency_ms, error, created_at FROM plugin_invocation
WHERE installation_id = $1
ORDER BY created_at DESC
LIMIT $2"#
    )
        .bind(installation_id)
        .bind(limit)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(PluginInvocation {
            id: row.try_get(0)?,
            installation_id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            hook_key: row.try_get(3)?,
            trigger: row.try_get(4)?,
            status: row.try_get(5)?,
            event_type: row.try_get(6)?,
            attempt: row.try_get(7)?,
            latency_ms: row.try_get(8)?,
            error: row.try_get(9)?,
            created_at: row.try_get(10)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListPluginSecretKeysRow {
    pub key: String,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn list_plugin_secret_keys(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<Vec<ListPluginSecretKeysRow>> {
    let rows = sqlx::query(
        r#"SELECT key, updated_at FROM plugin_secret
WHERE installation_id = $1
ORDER BY key ASC"#,
    )
    .bind(installation_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPluginSecretKeysRow {
            key: row.try_get(0)?,
            updated_at: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn list_plugin_skills(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    plugin_installation_id: Uuid,
) -> anyhow::Result<Vec<Skill>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id FROM skill WHERE plugin_installation_id = $1 ORDER BY name ASC"#
    )
        .bind(plugin_installation_id)
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
pub struct ListPluginStorageKeysRow {
    pub key: String,
    pub size_bytes: i64,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn list_plugin_storage_keys(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
) -> anyhow::Result<Vec<ListPluginStorageKeysRow>> {
    let rows = sqlx::query(
        r#"SELECT key, octet_length(value)::bigint AS size_bytes, updated_at
FROM plugin_storage
WHERE installation_id = $1 AND scope_type = $2 AND scope_id = $3
ORDER BY key ASC"#,
    )
    .bind(installation_id)
    .bind(scope_type)
    .bind(scope_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPluginStorageKeysRow {
            key: row.try_get(0)?,
            size_bytes: row.try_get(1)?,
            updated_at: row.try_get(2)?,
        });
    }
    Ok(out)
}

pub async fn list_workspace_plugin_installations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<PluginInstallation>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals FROM plugin_installation
WHERE workspace_id = $1
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(PluginInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            plugin_key: row.try_get(2)?,
            source_url: row.try_get(3)?,
            version: row.try_get(4)?,
            manifest: row.try_get(5)?,
            granted_scopes: row.try_get(6)?,
            config: row.try_get(7)?,
            enabled: row.try_get(8)?,
            installed_by: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
            token_hash: row.try_get(12)?,
            token_rotated_at: row.try_get(13)?,
            mcp_approvals: row.try_get(14)?,
        });
    }
    Ok(out)
}

pub async fn set_plugin_installation_enabled(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    enabled: bool,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"UPDATE plugin_installation
SET enabled = $2,
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals"#
    )
        .bind(id)
        .bind(enabled)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn set_plugin_installation_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    token_hash: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE plugin_installation
SET token_hash = $2, token_rotated_at = now(), updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(token_hash)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn set_plugin_mcp_approvals(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    mcp_approvals: &serde_json::Value,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"UPDATE plugin_installation
SET mcp_approvals = $2, updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals"#
    )
        .bind(id)
        .bind(mcp_approvals)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn update_plugin_installation_config(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    config: &serde_json::Value,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"UPDATE plugin_installation
SET config = $2,
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals"#
    )
        .bind(id)
        .bind(config)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn update_plugin_installation_manifest(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    source_url: &str,
    version: &str,
    manifest: &serde_json::Value,
    granted_scopes: &serde_json::Value,
    config: &serde_json::Value,
) -> anyhow::Result<Option<PluginInstallation>> {
    let row = sqlx::query(
        r#"UPDATE plugin_installation
SET source_url = $2,
    version = $3,
    manifest = $4,
    granted_scopes = $5,
    config = $6,
    updated_at = now()
WHERE id = $1
RETURNING id, workspace_id, plugin_key, source_url, version, manifest, granted_scopes, config, enabled, installed_by, created_at, updated_at, token_hash, token_rotated_at, mcp_approvals"#
    )
        .bind(id)
        .bind(source_url)
        .bind(version)
        .bind(manifest)
        .bind(granted_scopes)
        .bind(config)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        plugin_key: row.try_get(2)?,
        source_url: row.try_get(3)?,
        version: row.try_get(4)?,
        manifest: row.try_get(5)?,
        granted_scopes: row.try_get(6)?,
        config: row.try_get(7)?,
        enabled: row.try_get(8)?,
        installed_by: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
        token_hash: row.try_get(12)?,
        token_rotated_at: row.try_get(13)?,
        mcp_approvals: row.try_get(14)?,
    }))
}

pub async fn upsert_plugin_secret(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    key: &str,
    // plugin_secret.ciphertext is BYTEA; the generator could not see the
    // column type through the sqlc param and defaulted to JSON.
    ciphertext: &[u8],
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO plugin_secret (installation_id, key, ciphertext)
VALUES ($1, $2, $3)
ON CONFLICT (installation_id, key)
DO UPDATE SET ciphertext = EXCLUDED.ciphertext, updated_at = now()"#,
    )
    .bind(installation_id)
    .bind(key)
    .bind(ciphertext)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn upsert_plugin_skill(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &str,
    content: &str,
    plugin_installation_id: Uuid,
    created_by: Uuid,
) -> anyhow::Result<Option<Skill>> {
    let row = sqlx::query(
        r#"INSERT INTO skill (workspace_id, name, description, content, config, created_by, plugin_installation_id)
VALUES ($1, $2, $3, $4, '{}'::jsonb, $6, $5)
ON CONFLICT (workspace_id, name) DO UPDATE SET
    description = EXCLUDED.description,
    content = EXCLUDED.content,
    updated_at = now()
WHERE skill.plugin_installation_id = EXCLUDED.plugin_installation_id
RETURNING id, workspace_id, name, description, content, config, created_by, created_at, updated_at, plugin_installation_id"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(content)
        .bind(plugin_installation_id)
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

pub async fn upsert_plugin_storage_value(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    key: &str,
    value: &str,
) -> anyhow::Result<Option<PluginStorage>> {
    let row = sqlx::query(
        r#"INSERT INTO plugin_storage (installation_id, scope_type, scope_id, key, value)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (installation_id, scope_type, scope_id, key)
DO UPDATE SET value = EXCLUDED.value, updated_at = now()
RETURNING id, installation_id, scope_type, scope_id, key, value, created_at, updated_at"#,
    )
    .bind(installation_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(key)
    .bind(value)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PluginStorage {
        id: row.try_get(0)?,
        installation_id: row.try_get(1)?,
        scope_type: row.try_get(2)?,
        scope_id: row.try_get(3)?,
        key: row.try_get(4)?,
        value: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}
