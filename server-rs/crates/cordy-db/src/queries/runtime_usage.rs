//! Typed SQL queries for runtime_usage records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetRuntimeTaskHourlyActivityRow {
    pub hour: i32,
    pub count: i32,
}

pub async fn get_runtime_task_hourly_activity(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    tz: &str,
) -> anyhow::Result<Vec<GetRuntimeTaskHourlyActivityRow>> {
    let rows = sqlx::query(
        r#"SELECT EXTRACT(HOUR FROM started_at AT TIME ZONE $2::text)::int AS hour,
       COUNT(*)::int AS count
FROM agent_task_queue
WHERE runtime_id = $1 AND started_at IS NOT NULL
GROUP BY hour
ORDER BY hour"#,
    )
    .bind(runtime_id)
    .bind(tz)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetRuntimeTaskHourlyActivityRow {
            hour: row.try_get(0)?,
            count: row.try_get(1)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetRuntimeUsageByHourRow {
    pub hour: i32,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd_ticks: i64,
    pub uncosted_input_tokens: i64,
    pub uncosted_output_tokens: i64,
    pub uncosted_cache_read_tokens: i64,
    pub uncosted_cache_write_tokens: i64,
    pub task_count: i32,
}

pub async fn get_runtime_usage_by_hour(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    tz: &str,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<GetRuntimeUsageByHourRow>> {
    let rows = sqlx::query(
        r#"SELECT
    EXTRACT(HOUR FROM tu.created_at AT TIME ZONE $2::text)::int AS hour,
    tu.model,
    SUM(tu.input_tokens)::bigint AS input_tokens,
    SUM(tu.output_tokens)::bigint AS output_tokens,
    SUM(tu.cache_read_tokens)::bigint AS cache_read_tokens,
    SUM(tu.cache_write_tokens)::bigint AS cache_write_tokens,
    COALESCE(SUM(tu.cost_usd_ticks), 0)::bigint AS cost_usd_ticks,
    COALESCE(SUM(tu.input_tokens)       FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_input_tokens,
    COALESCE(SUM(tu.output_tokens)      FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_output_tokens,
    COALESCE(SUM(tu.cache_read_tokens)  FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_cache_read_tokens,
    COALESCE(SUM(tu.cache_write_tokens) FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_cache_write_tokens,
    COUNT(DISTINCT tu.task_id)::int AS task_count
FROM task_usage tu
JOIN agent_task_queue atq ON atq.id = tu.task_id
WHERE atq.runtime_id = $1
  AND tu.created_at >= $3::timestamptz
GROUP BY EXTRACT(HOUR FROM tu.created_at AT TIME ZONE $2::text), tu.model
ORDER BY hour, tu.model"#
    )
        .bind(runtime_id)
        .bind(tz)
        .bind(since)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetRuntimeUsageByHourRow {
            hour: row.try_get(0)?,
            model: row.try_get(1)?,
            input_tokens: row.try_get(2)?,
            output_tokens: row.try_get(3)?,
            cache_read_tokens: row.try_get(4)?,
            cache_write_tokens: row.try_get(5)?,
            cost_usd_ticks: row.try_get(6)?,
            uncosted_input_tokens: row.try_get(7)?,
            uncosted_output_tokens: row.try_get(8)?,
            uncosted_cache_read_tokens: row.try_get(9)?,
            uncosted_cache_write_tokens: row.try_get(10)?,
            task_count: row.try_get(11)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListRuntimeUsageRow {
    pub date: Option<chrono::NaiveDate>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd_ticks: i64,
    pub uncosted_input_tokens: i64,
    pub uncosted_output_tokens: i64,
    pub uncosted_cache_read_tokens: i64,
    pub uncosted_cache_write_tokens: i64,
}

pub async fn list_runtime_usage(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    tz: &str,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<ListRuntimeUsageRow>> {
    let rows = sqlx::query(
        r#"SELECT
    DATE(bucket_hour AT TIME ZONE $2::text) AS date,
    LOWER(provider) AS provider,
    model,
    SUM(input_tokens)::bigint        AS input_tokens,
    SUM(output_tokens)::bigint       AS output_tokens,
    SUM(cache_read_tokens)::bigint   AS cache_read_tokens,
    SUM(cache_write_tokens)::bigint  AS cache_write_tokens,
    SUM(cost_usd_ticks)::bigint                                          AS cost_usd_ticks,
    SUM(COALESCE(uncosted_input_tokens, input_tokens))::bigint           AS uncosted_input_tokens,
    SUM(COALESCE(uncosted_output_tokens, output_tokens))::bigint         AS uncosted_output_tokens,
    SUM(COALESCE(uncosted_cache_read_tokens, cache_read_tokens))::bigint AS uncosted_cache_read_tokens,
    SUM(COALESCE(uncosted_cache_write_tokens, cache_write_tokens))::bigint AS uncosted_cache_write_tokens
FROM task_usage_hourly
WHERE runtime_id = $1
  AND bucket_hour >= $3::timestamptz
GROUP BY DATE(bucket_hour AT TIME ZONE $2::text), LOWER(provider), model
ORDER BY DATE(bucket_hour AT TIME ZONE $2::text) DESC, LOWER(provider), model"#
    )
        .bind(runtime_id)
        .bind(tz)
        .bind(since)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListRuntimeUsageRow {
            date: row.try_get(0)?,
            provider: row.try_get(1)?,
            model: row.try_get(2)?,
            input_tokens: row.try_get(3)?,
            output_tokens: row.try_get(4)?,
            cache_read_tokens: row.try_get(5)?,
            cache_write_tokens: row.try_get(6)?,
            cost_usd_ticks: row.try_get(7)?,
            uncosted_input_tokens: row.try_get(8)?,
            uncosted_output_tokens: row.try_get(9)?,
            uncosted_cache_read_tokens: row.try_get(10)?,
            uncosted_cache_write_tokens: row.try_get(11)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListRuntimeUsageByAgentRow {
    pub agent_id: Option<Uuid>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd_ticks: i64,
    pub uncosted_input_tokens: i64,
    pub uncosted_output_tokens: i64,
    pub uncosted_cache_read_tokens: i64,
    pub uncosted_cache_write_tokens: i64,
    pub task_count: i32,
}

pub async fn list_runtime_usage_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<ListRuntimeUsageByAgentRow>> {
    let rows = sqlx::query(
        r#"SELECT
    atq.agent_id,
    LOWER(tu.provider) AS provider,
    tu.model,
    SUM(tu.input_tokens)::bigint AS input_tokens,
    SUM(tu.output_tokens)::bigint AS output_tokens,
    SUM(tu.cache_read_tokens)::bigint AS cache_read_tokens,
    SUM(tu.cache_write_tokens)::bigint AS cache_write_tokens,
    COALESCE(SUM(tu.cost_usd_ticks), 0)::bigint AS cost_usd_ticks,
    COALESCE(SUM(tu.input_tokens)       FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_input_tokens,
    COALESCE(SUM(tu.output_tokens)      FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_output_tokens,
    COALESCE(SUM(tu.cache_read_tokens)  FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_cache_read_tokens,
    COALESCE(SUM(tu.cache_write_tokens) FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_cache_write_tokens,
    COUNT(DISTINCT tu.task_id)::int AS task_count
FROM task_usage tu
JOIN agent_task_queue atq ON atq.id = tu.task_id
WHERE atq.runtime_id = $1
  AND tu.created_at >= $2::timestamptz
GROUP BY atq.agent_id, LOWER(tu.provider), tu.model
ORDER BY atq.agent_id, LOWER(tu.provider), tu.model"#
    )
        .bind(runtime_id)
        .bind(since)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListRuntimeUsageByAgentRow {
            agent_id: row.try_get(0)?,
            provider: row.try_get(1)?,
            model: row.try_get(2)?,
            input_tokens: row.try_get(3)?,
            output_tokens: row.try_get(4)?,
            cache_read_tokens: row.try_get(5)?,
            cache_write_tokens: row.try_get(6)?,
            cost_usd_ticks: row.try_get(7)?,
            uncosted_input_tokens: row.try_get(8)?,
            uncosted_output_tokens: row.try_get(9)?,
            uncosted_cache_read_tokens: row.try_get(10)?,
            uncosted_cache_write_tokens: row.try_get(11)?,
            task_count: row.try_get(12)?,
        });
    }
    Ok(out)
}
