//! Port of server/pkg/db/queries/task_usage.sql (generated task_usage.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetIssueUsageSummaryRow {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_write_tokens: i64,
    pub total_cost_usd_ticks: i64,
    pub uncosted_input_tokens: i64,
    pub uncosted_output_tokens: i64,
    pub uncosted_cache_read_tokens: i64,
    pub uncosted_cache_write_tokens: i64,
    pub task_count: i32,
}

pub async fn get_issue_usage_summary(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<GetIssueUsageSummaryRow>> {
    let row = sqlx::query(
        r#"SELECT
    COALESCE(SUM(tu.input_tokens), 0)::bigint AS total_input_tokens,
    COALESCE(SUM(tu.output_tokens), 0)::bigint AS total_output_tokens,
    COALESCE(SUM(tu.cache_read_tokens), 0)::bigint AS total_cache_read_tokens,
    COALESCE(SUM(tu.cache_write_tokens), 0)::bigint AS total_cache_write_tokens,
    COALESCE(SUM(tu.cost_usd_ticks), 0)::bigint AS total_cost_usd_ticks,
    COALESCE(SUM(tu.input_tokens)       FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_input_tokens,
    COALESCE(SUM(tu.output_tokens)      FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_output_tokens,
    COALESCE(SUM(tu.cache_read_tokens)  FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_cache_read_tokens,
    COALESCE(SUM(tu.cache_write_tokens) FILTER (WHERE tu.cost_usd_ticks IS NULL), 0)::bigint AS uncosted_cache_write_tokens,
    COUNT(DISTINCT tu.task_id)::int AS task_count
FROM task_usage tu
JOIN agent_task_queue atq ON atq.id = tu.task_id
WHERE atq.issue_id = $1"#
    )
        .bind(issue_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetIssueUsageSummaryRow {
        total_input_tokens: row.try_get(0)?,
        total_output_tokens: row.try_get(1)?,
        total_cache_read_tokens: row.try_get(2)?,
        total_cache_write_tokens: row.try_get(3)?,
        total_cost_usd_ticks: row.try_get(4)?,
        uncosted_input_tokens: row.try_get(5)?,
        uncosted_output_tokens: row.try_get(6)?,
        uncosted_cache_read_tokens: row.try_get(7)?,
        uncosted_cache_write_tokens: row.try_get(8)?,
        task_count: row.try_get(9)?,
    }))
}

pub async fn get_task_usage(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Vec<TaskUsage>> {
    let rows = sqlx::query(
        r#"SELECT id, task_id, provider, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, created_at, updated_at, cost_usd_ticks FROM task_usage
WHERE task_id = $1
ORDER BY model"#
    )
        .bind(task_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(TaskUsage {
            id: row.try_get(0)?,
            task_id: row.try_get(1)?,
            provider: row.try_get(2)?,
            model: row.try_get(3)?,
            input_tokens: row.try_get(4)?,
            output_tokens: row.try_get(5)?,
            cache_read_tokens: row.try_get(6)?,
            cache_write_tokens: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
            cost_usd_ticks: row.try_get(10)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDashboardAgentRunTimeRow {
    pub agent_id: Option<Uuid>,
    pub total_seconds: i64,
    pub task_count: i32,
    pub failed_count: i32,
    pub cancelled_count: i32,
}

pub async fn list_dashboard_agent_run_time(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    since: Option<DateTime<Utc>>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ListDashboardAgentRunTimeRow>> {
    let rows = sqlx::query(
        r#"SELECT
    atq.agent_id,
    COALESCE(
        SUM(EXTRACT(EPOCH FROM (atq.completed_at - atq.started_at)))::bigint,
        0
    )::bigint AS total_seconds,
    COUNT(*)::int AS task_count,
    COUNT(*) FILTER (WHERE atq.status = 'failed')::int AS failed_count,
    COUNT(*) FILTER (WHERE atq.status = 'cancelled')::int AS cancelled_count
FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
LEFT JOIN issue i ON i.id = atq.issue_id
WHERE a.workspace_id = $1
  AND atq.status IN ('completed', 'failed', 'cancelled')
  AND atq.started_at IS NOT NULL
  AND atq.completed_at IS NOT NULL
  AND atq.completed_at >= $2::timestamptz
  AND ($3::uuid IS NULL OR i.project_id = $3)
GROUP BY atq.agent_id
ORDER BY total_seconds DESC"#,
    )
    .bind(workspace_id)
    .bind(since)
    .bind(project_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDashboardAgentRunTimeRow {
            agent_id: row.try_get(0)?,
            total_seconds: row.try_get(1)?,
            task_count: row.try_get(2)?,
            failed_count: row.try_get(3)?,
            cancelled_count: row.try_get(4)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDashboardFailuresByAgentRow {
    pub agent_id: Option<Uuid>,
    pub failure_reason: String,
    pub task_count: i32,
}

pub async fn list_dashboard_failures_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    since: Option<DateTime<Utc>>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ListDashboardFailuresByAgentRow>> {
    let rows = sqlx::query(
        r#"SELECT
    atq.agent_id,
    CASE
        WHEN atq.status = 'failed'
            THEN COALESCE(NULLIF(atq.failure_reason, ''), 'unclassified')
        ELSE ''
    END AS failure_reason,
    COUNT(*)::int AS task_count
FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
LEFT JOIN issue i ON i.id = atq.issue_id
WHERE a.workspace_id = $1
  AND atq.status IN ('completed', 'failed')
  AND atq.completed_at IS NOT NULL
  AND atq.completed_at >= $2::timestamptz
  AND ($3::uuid IS NULL OR i.project_id = $3)
GROUP BY atq.agent_id, 2
ORDER BY atq.agent_id, 2"#,
    )
    .bind(workspace_id)
    .bind(since)
    .bind(project_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDashboardFailuresByAgentRow {
            agent_id: row.try_get(0)?,
            failure_reason: row.try_get(1)?,
            task_count: row.try_get(2)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDashboardFailuresDailyRow {
    pub date: Option<chrono::NaiveDate>,
    pub failure_reason: String,
    pub task_count: i32,
}

pub async fn list_dashboard_failures_daily(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    tz: &str,
    since: Option<DateTime<Utc>>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ListDashboardFailuresDailyRow>> {
    let rows = sqlx::query(
        r#"SELECT
    DATE(atq.completed_at AT TIME ZONE $2::text) AS date,
    CASE
        WHEN atq.status = 'failed'
            THEN COALESCE(NULLIF(atq.failure_reason, ''), 'unclassified')
        ELSE ''
    END AS failure_reason,
    COUNT(*)::int AS task_count
FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
LEFT JOIN issue i ON i.id = atq.issue_id
WHERE a.workspace_id = $1
  AND atq.status IN ('completed', 'failed')
  AND atq.completed_at IS NOT NULL
  AND atq.completed_at >= $3::timestamptz
  AND ($4::uuid IS NULL OR i.project_id = $4)
GROUP BY 1, 2
ORDER BY 1 DESC, 2"#,
    )
    .bind(workspace_id)
    .bind(tz)
    .bind(since)
    .bind(project_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDashboardFailuresDailyRow {
            date: row.try_get(0)?,
            failure_reason: row.try_get(1)?,
            task_count: row.try_get(2)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDashboardRunTimeDailyRow {
    pub date: Option<chrono::NaiveDate>,
    pub total_seconds: i64,
    pub task_count: i32,
    pub failed_count: i32,
    pub cancelled_count: i32,
}

pub async fn list_dashboard_run_time_daily(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    tz: &str,
    since: Option<DateTime<Utc>>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ListDashboardRunTimeDailyRow>> {
    let rows = sqlx::query(
        r#"SELECT
    DATE(atq.completed_at AT TIME ZONE $2::text) AS date,
    COALESCE(
        SUM(EXTRACT(EPOCH FROM (atq.completed_at - atq.started_at)))::bigint,
        0
    )::bigint AS total_seconds,
    COUNT(*)::int AS task_count,
    COUNT(*) FILTER (WHERE atq.status = 'failed')::int AS failed_count,
    COUNT(*) FILTER (WHERE atq.status = 'cancelled')::int AS cancelled_count
FROM agent_task_queue atq
JOIN agent a ON a.id = atq.agent_id
LEFT JOIN issue i ON i.id = atq.issue_id
WHERE a.workspace_id = $1
  AND atq.status IN ('completed', 'failed', 'cancelled')
  AND atq.started_at IS NOT NULL
  AND atq.completed_at IS NOT NULL
  AND atq.completed_at >= $3::timestamptz
  AND ($4::uuid IS NULL OR i.project_id = $4)
GROUP BY DATE(atq.completed_at AT TIME ZONE $2::text)
ORDER BY DATE(atq.completed_at AT TIME ZONE $2::text) DESC"#,
    )
    .bind(workspace_id)
    .bind(tz)
    .bind(since)
    .bind(project_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDashboardRunTimeDailyRow {
            date: row.try_get(0)?,
            total_seconds: row.try_get(1)?,
            task_count: row.try_get(2)?,
            failed_count: row.try_get(3)?,
            cancelled_count: row.try_get(4)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDashboardUsageByAgentRow {
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

pub async fn list_dashboard_usage_by_agent(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    since: Option<DateTime<Utc>>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ListDashboardUsageByAgentRow>> {
    let rows = sqlx::query(
        r#"SELECT
    agent_id,
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
    SUM(COALESCE(uncosted_cache_write_tokens, cache_write_tokens))::bigint AS uncosted_cache_write_tokens,
    SUM(task_count)::int             AS task_count
FROM task_usage_hourly
WHERE workspace_id = $1
  AND bucket_hour >= $2::timestamptz
  AND ($3::uuid IS NULL OR project_id = $3)
GROUP BY agent_id, LOWER(provider), model
ORDER BY agent_id, LOWER(provider), model"#
    )
        .bind(workspace_id)
        .bind(since)
        .bind(project_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDashboardUsageByAgentRow {
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListDashboardUsageDailyRow {
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
    pub task_count: i32,
}

pub async fn list_dashboard_usage_daily(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    tz: &str,
    since: Option<DateTime<Utc>>,
    project_id: Uuid,
) -> anyhow::Result<Vec<ListDashboardUsageDailyRow>> {
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
    SUM(COALESCE(uncosted_cache_write_tokens, cache_write_tokens))::bigint AS uncosted_cache_write_tokens,
    SUM(task_count)::int             AS task_count
FROM task_usage_hourly
WHERE workspace_id = $1
  AND bucket_hour >= $3::timestamptz
  AND ($4::uuid IS NULL OR project_id = $4)
GROUP BY DATE(bucket_hour AT TIME ZONE $2::text), LOWER(provider), model
ORDER BY DATE(bucket_hour AT TIME ZONE $2::text) DESC, LOWER(provider), model"#
    )
        .bind(workspace_id)
        .bind(tz)
        .bind(since)
        .bind(project_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListDashboardUsageDailyRow {
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
            task_count: row.try_get(12)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListIssueTaskUsageRow {
    pub task_id: Option<Uuid>,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd_ticks: Option<i64>,
}

pub async fn list_issue_task_usage(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<ListIssueTaskUsageRow>> {
    let rows = sqlx::query(
        r#"SELECT
    tu.task_id,
    tu.provider,
    tu.model,
    tu.input_tokens,
    tu.output_tokens,
    tu.cache_read_tokens,
    tu.cache_write_tokens,
    tu.cost_usd_ticks
FROM task_usage tu
JOIN agent_task_queue atq ON atq.id = tu.task_id
WHERE atq.issue_id = $1
ORDER BY tu.task_id, tu.model"#,
    )
    .bind(issue_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListIssueTaskUsageRow {
            task_id: row.try_get(0)?,
            provider: row.try_get(1)?,
            model: row.try_get(2)?,
            input_tokens: row.try_get(3)?,
            output_tokens: row.try_get(4)?,
            cache_read_tokens: row.try_get(5)?,
            cache_write_tokens: row.try_get(6)?,
            cost_usd_ticks: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn upsert_task_usage(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    provider: &str,
    model: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_usd_ticks: Option<i64>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO task_usage (task_id, provider, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd_ticks, updated_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
ON CONFLICT (task_id, provider, model)
DO UPDATE SET
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_write_tokens = EXCLUDED.cache_write_tokens,
    cost_usd_ticks = EXCLUDED.cost_usd_ticks,
    updated_at = now()"#
    )
        .bind(task_id)
        .bind(provider)
        .bind(model)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cache_read_tokens)
        .bind(cache_write_tokens)
        .bind(cost_usd_ticks)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}
