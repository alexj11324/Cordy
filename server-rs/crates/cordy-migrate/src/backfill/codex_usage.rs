//! Repairs historical Codex `task_usage.input_tokens` rows that were written
//! before cached input was normalized at ingestion time.
//!
//! Port of `server/cmd/backfill_codex_usage_cache`. The command is a hosted
//! data repair: dry-run is the default, `--cutoff` is required, and only rows
//! that are still before the cutoff can be updated.

use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

const ROLLUP_ADVISORY_LOCK_ID: i64 = 4246;

#[derive(Debug, Clone)]
pub struct OperatorOptions {
    pub cutoff: DateTime<Utc>,
    pub workspace_id: String,
    pub batch_size: i64,
    pub sleep_between_batches: Duration,
    pub execute: bool,
    pub rebuild_rollup: bool,
}

#[derive(Default)]
struct Totals {
    rows: i64,
    input_before: i64,
    input_after: i64,
    overcount: i64,
    clamped_rows: i64,
}

struct SummaryRow {
    workspace_id: String,
    date_utc: String,
    rows: i64,
    input_before: i64,
    input_after: i64,
    overcount: i64,
    clamped_rows: i64,
    min_created_at: DateTime<Utc>,
    max_created_at: DateTime<Utc>,
}

/// Runs the dry-run/execute Codex cache repair and optional hourly rollup
/// rebuild under advisory lock 4246.
pub async fn run_operator(
    pool: &PgPool,
    options: OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    let (summary, totals) = load_dry_run_summary(pool, &options, cancellation).await?;
    log_summary(&options, &summary, &totals);
    if totals.rows == 0 {
        tracing::info!("no eligible Codex task_usage rows found");
        return Ok(());
    }
    if !options.execute {
        tracing::info!(
            "dry-run complete; review the summary, then re-run with --execute to apply the backfill"
        );
        return Ok(());
    }

    let mut lock_conn = cancellable(cancellation, pool.acquire()).await?;
    cancellable(
        cancellation,
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(ROLLUP_ADVISORY_LOCK_ID)
            .execute(&mut *lock_conn),
    )
    .await?;

    let result = execute_locked(pool, &options, cancellation).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ROLLUP_ADVISORY_LOCK_ID)
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);

    match result {
        Ok(()) => {
            unlock_result?;
            Ok(())
        }
        Err(error) => {
            if let Err(unlock_error) = unlock_result {
                tracing::warn!(error = %unlock_error, "Codex usage cache backfill: advisory unlock failed after run error");
            }
            Err(error)
        }
    }
}

async fn load_dry_run_summary(
    pool: &PgPool,
    options: &OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<(Vec<SummaryRow>, Totals)> {
    let mut rows = sqlx::query_as::<_, (
        String,
        String,
        i64,
        i64,
        i64,
        i64,
        i64,
        DateTime<Utc>,
        DateTime<Utc>,
    )>(
        r#"
SELECT
    a.workspace_id::text AS workspace_id,
    (tu.created_at AT TIME ZONE 'UTC')::date::text AS date_utc,
    COUNT(*)::bigint AS rows,
    COALESCE(SUM(tu.input_tokens), 0)::bigint AS input_before,
    COALESCE(SUM(GREATEST(tu.input_tokens - tu.cache_read_tokens, 0)), 0)::bigint AS input_after,
    COALESCE(SUM(tu.input_tokens - GREATEST(tu.input_tokens - tu.cache_read_tokens, 0)), 0)::bigint AS overcount,
    COUNT(*) FILTER (WHERE tu.input_tokens < tu.cache_read_tokens)::bigint AS clamped_rows,
    MIN(tu.created_at) AS min_created_at,
    MAX(tu.created_at) AS max_created_at
FROM task_usage tu
JOIN agent_task_queue atq ON atq.id = tu.task_id
JOIN agent a ON a.id = atq.agent_id
WHERE tu.provider = 'codex'
  AND tu.cache_read_tokens > 0
  AND tu.input_tokens > 0
  AND COALESCE(tu.updated_at, tu.created_at) < $1
  AND (NULLIF($2, '')::uuid IS NULL OR a.workspace_id = NULLIF($2, '')::uuid)
GROUP BY a.workspace_id, (tu.created_at AT TIME ZONE 'UTC')::date
ORDER BY a.workspace_id, date_utc
        "#,
    )
    .bind(options.cutoff)
    .bind(&options.workspace_id)
    .fetch(pool);

    let mut summary = Vec::new();
    let mut totals = Totals::default();
    while let Some(row) = tokio::select! {
        _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
        row = rows.try_next() => row?,
    } {
        let item = SummaryRow {
            workspace_id: row.0,
            date_utc: row.1,
            rows: row.2,
            input_before: row.3,
            input_after: row.4,
            overcount: row.5,
            clamped_rows: row.6,
            min_created_at: row.7,
            max_created_at: row.8,
        };
        totals.rows += item.rows;
        totals.input_before += item.input_before;
        totals.input_after += item.input_after;
        totals.overcount += item.overcount;
        totals.clamped_rows += item.clamped_rows;
        summary.push(item);
    }
    Ok((summary, totals))
}

fn log_summary(options: &OperatorOptions, rows: &[SummaryRow], totals: &Totals) {
    tracing::info!(
        execute = options.execute,
        cutoff = %options.cutoff.to_rfc3339(),
        workspace_id = %options.workspace_id,
        rows = totals.rows,
        input_before = totals.input_before,
        input_after = totals.input_after,
        input_tokens_removed = totals.overcount,
        clamped_rows = totals.clamped_rows,
        "Codex usage cache backfill candidate total"
    );
    for row in rows {
        tracing::info!(
            workspace_id = %row.workspace_id,
            date_utc = %row.date_utc,
            rows = row.rows,
            input_before = row.input_before,
            input_after = row.input_after,
            input_tokens_removed = row.overcount,
            clamped_rows = row.clamped_rows,
            min_created_at = %row.min_created_at.to_rfc3339(),
            max_created_at = %row.max_created_at.to_rfc3339(),
            "Codex usage cache backfill candidate summary"
        );
    }
}

async fn execute_locked(
    pool: &PgPool,
    options: &OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    let update_started_at = cancellable(cancellation, database_clock(pool)).await?;
    let (updated_rows, removed_tokens) = execute_backfill(pool, options, cancellation).await?;
    tracing::info!(
        rows = updated_rows,
        input_tokens_removed = removed_tokens,
        "task_usage rows updated"
    );
    if updated_rows == 0 || !options.rebuild_rollup {
        return Ok(());
    }

    let update_finished_at = cancellable(cancellation, database_clock(pool)).await?;
    let (rollup_from, rollup_to) = rollup_window(update_started_at, update_finished_at);
    let rollup_rows: i64 = cancellable(
        cancellation,
        sqlx::query_scalar(
            "SELECT rollup_task_usage_hourly_window($1::timestamptz, $2::timestamptz)",
        )
        .bind(rollup_from)
        .bind(rollup_to)
        .fetch_one(pool),
    )
    .await
    .map_err(|error| {
        anyhow::anyhow!(
            "rebuild hourly rollup for update window {rollup_from}..{rollup_to}: {error}"
        )
    })?;
    tracing::info!(
        from = %rollup_from,
        to = %rollup_to,
        rows_touched = rollup_rows,
        "hourly rollup rebuilt"
    );
    Ok(())
}

async fn execute_backfill(
    pool: &PgPool,
    options: &OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<(i64, i64)> {
    let query = r#"
WITH candidates AS (
    SELECT
        tu.id,
        (tu.input_tokens - GREATEST(tu.input_tokens - tu.cache_read_tokens, 0))::bigint AS removed_tokens
      FROM task_usage tu
      JOIN agent_task_queue atq ON atq.id = tu.task_id
      JOIN agent a ON a.id = atq.agent_id
     WHERE tu.provider = 'codex'
       AND tu.cache_read_tokens > 0
       AND tu.input_tokens > 0
       AND COALESCE(tu.updated_at, tu.created_at) < $1
       AND (NULLIF($2, '')::uuid IS NULL OR a.workspace_id = NULLIF($2, '')::uuid)
     ORDER BY COALESCE(tu.updated_at, tu.created_at), tu.id
     LIMIT $3
     FOR UPDATE OF tu SKIP LOCKED
),
updated AS (
    UPDATE task_usage tu
       SET input_tokens = GREATEST(tu.input_tokens - tu.cache_read_tokens, 0),
           updated_at = now()
      FROM candidates c
     WHERE tu.id = c.id
     RETURNING c.removed_tokens
)
SELECT COUNT(*)::bigint, COALESCE(SUM(removed_tokens), 0)::bigint FROM updated
    "#;

    let mut total_rows = 0_i64;
    let mut total_removed = 0_i64;
    loop {
        let (rows, removed): (i64, i64) = cancellable(
            cancellation,
            sqlx::query_as(query)
                .bind(options.cutoff)
                .bind(&options.workspace_id)
                .bind(options.batch_size)
                .fetch_one(pool),
        )
        .await
        .map_err(|error| anyhow::anyhow!("update Codex task_usage batch: {error}"))?;
        if rows == 0 {
            break;
        }
        total_rows += rows;
        total_removed += removed;
        tracing::info!(
            rows,
            input_tokens_removed = removed,
            total_rows,
            "updated Codex task_usage batch"
        );
        if options.sleep_between_batches > Duration::ZERO {
            tokio::select! {
                _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
                _ = tokio::time::sleep(options.sleep_between_batches) => {},
            }
        }
    }
    Ok((total_rows, total_removed))
}

async fn database_clock(pool: &PgPool) -> Result<DateTime<Utc>, sqlx::Error> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await
}

fn rollup_window(
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        started_at - chrono::Duration::seconds(1),
        finished_at + chrono::Duration::seconds(1),
    )
}

async fn cancellable<T, F>(cancellation: &CancellationToken, future: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    tokio::select! {
        _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
        result = future => Ok(result?),
    }
}
