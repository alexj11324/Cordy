//! Port of server/pkg/db/queries/client_usage.sql (generated client_usage.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn upsert_client_usage_daily(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    client_type: &str,
    install_id: Uuid,
    workspace_id: Option<Uuid>,
    client_version: &str,
    os: &str,
    has_runtime_probe: bool,
    probe_result: Option<&str>,
    runtime_count: Option<i32>,
    provider_summary: Option<&serde_json::Value>,
    online_count: Option<i32>,
    offline_count: Option<i32>,
) -> anyhow::Result<Option<ClientUsageDaily>> {
    let row = sqlx::query(
        r#"INSERT INTO client_usage_daily (
    user_id,
    client_type,
    install_id,
    activity_date,
    workspace_id,
    client_version,
    os,
    first_active_at,
    last_active_at,
    runtime_probed_at,
    probe_result,
    runtime_count,
    provider_summary,
    online_count,
    offline_count
) VALUES (
    $1,
    $2,
    $3,
    (CURRENT_TIMESTAMP AT TIME ZONE 'UTC')::date,
    $4,
    $5,
    $6,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP,
    CASE WHEN $7::boolean THEN CURRENT_TIMESTAMP ELSE NULL END,
    $8,
    $9,
    $10,
    $11,
    $12
)
ON CONFLICT (user_id, client_type, install_id, activity_date) DO UPDATE SET
    workspace_id = COALESCE(EXCLUDED.workspace_id, client_usage_daily.workspace_id),
    client_version = EXCLUDED.client_version,
    os = EXCLUDED.os,
    last_active_at = EXCLUDED.last_active_at,
    runtime_probed_at = CASE WHEN $7::boolean THEN EXCLUDED.runtime_probed_at ELSE client_usage_daily.runtime_probed_at END,
    probe_result = CASE WHEN $7::boolean THEN EXCLUDED.probe_result ELSE client_usage_daily.probe_result END,
    runtime_count = CASE WHEN $7::boolean THEN EXCLUDED.runtime_count ELSE client_usage_daily.runtime_count END,
    provider_summary = CASE WHEN $7::boolean THEN EXCLUDED.provider_summary ELSE client_usage_daily.provider_summary END,
    online_count = CASE WHEN $7::boolean THEN EXCLUDED.online_count ELSE client_usage_daily.online_count END,
    offline_count = CASE WHEN $7::boolean THEN EXCLUDED.offline_count ELSE client_usage_daily.offline_count END,
    updated_at = CURRENT_TIMESTAMP
RETURNING user_id, client_type, install_id, activity_date, workspace_id, client_version, os, first_active_at, last_active_at, runtime_probed_at, probe_result, runtime_count, provider_summary, online_count, offline_count, created_at, updated_at"#
    )
        .bind(user_id)
        .bind(client_type)
        .bind(install_id)
        .bind(workspace_id)
        .bind(client_version)
        .bind(os)
        .bind(has_runtime_probe)
        .bind(probe_result)
        .bind(runtime_count)
        .bind(provider_summary)
        .bind(online_count)
        .bind(offline_count)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ClientUsageDaily {
        user_id: row.try_get(0)?,
        client_type: row.try_get(1)?,
        install_id: row.try_get(2)?,
        activity_date: row.try_get(3)?,
        workspace_id: row.try_get(4)?,
        client_version: row.try_get(5)?,
        os: row.try_get(6)?,
        first_active_at: row.try_get(7)?,
        last_active_at: row.try_get(8)?,
        runtime_probed_at: row.try_get(9)?,
        probe_result: row.try_get(10)?,
        runtime_count: row.try_get(11)?,
        provider_summary: row.try_get(12)?,
        online_count: row.try_get(13)?,
        offline_count: row.try_get(14)?,
        created_at: row.try_get(15)?,
        updated_at: row.try_get(16)?,
    }))
}
