//! Seeds `task_usage_hourly` from historical `task_usage` rows before
//! migration 103's fail-closed watermark guard (MUL-2957).
//!
//! Port of `server/internal/taskusagebackfill/backfill.go`. The hook does NOT
//! fail when task_usage is empty (fresh DB — the watermark is stamped so
//! migration 103 accepts the empty state) or when the rollup state tables are
//! missing (migrations 101/102 not yet applied). It DOES fail when the rollup
//! walk itself errors, aborting the migrate run.

use std::{future::Future, time::Duration};

use chrono::{DateTime, Datelike, Months, TimeZone, Utc};
use sqlx::{PgConnection, PgPool};
use tokio_util::sync::CancellationToken;

/// Shared with rollup_task_usage_hourly(), the standalone backfill command,
/// and the in-process scheduler so a mixed-version cluster cannot double-write.
const ADVISORY_LOCK_KEY: i64 = 4246;

/// Mirrors migration 103's v_lag interval. Below this threshold the migration
/// would have passed anyway, so we save the scan.
const MAX_LAG_THRESHOLD_SECS: i64 = 3600;

/// Options for the operator-facing historical hourly rollup backfill.
///
/// This is the Rust equivalent of the flags accepted by
/// `server/cmd/backfill_task_usage_hourly`.
#[derive(Debug, Clone, Default)]
pub struct OperatorOptions {
    pub dry_run: bool,
    pub months_back: i64,
    pub force_partial: bool,
    pub sleep_between_slices: Duration,
}

enum OperatorOutcome {
    Empty,
    DryRun,
    Complete { rows_touched: i64 },
}

struct UsageRange {
    min_event: Option<DateTime<Utc>>,
    max_event: Option<DateTime<Utc>>,
}

pub(crate) async fn hook(pool: &PgPool) -> anyhow::Result<()> {
    // Step 1: cheap precondition — no rollup state tables yet means nothing to do.
    let state_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public'
               AND table_name = 'task_usage_hourly_rollup_state'
        )
        "#,
    )
    .fetch_one(pool)
    .await?;
    if !state_exists {
        tracing::info!(
            reason = "migrations 101/102 not yet applied",
            "task_usage hourly rollup hook: rollup state tables not present, skipping"
        );
        return Ok(());
    }

    // Step 2: read range and watermark on the pool; only used to decide whether
    // the lock-protected walk should run. COALESCE(updated_at, created_at)
    // tracks the same expression migration 103's guard uses.
    let (min_ts, max_ts): (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT MIN(created_at), MAX(COALESCE(updated_at, created_at)) FROM task_usage",
    )
    .fetch_one(pool)
    .await?;
    let usage_range = UsageRange {
        min_event: min_ts,
        max_event: max_ts,
    };

    let watermark: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT watermark_at FROM task_usage_hourly_rollup_state WHERE id = 1")
            .fetch_optional(pool)
            .await?;

    let Some(max_event) = usage_range.max_event else {
        // Empty database: stamp the watermark from the DB's own clock so a
        // clock-skewed app process cannot stamp into the DB's future.
        stamp_watermark(pool).await?;
        tracing::info!(
            "task_usage hourly rollup hook: task_usage empty, watermark stamped from db now()"
        );
        return Ok(());
    };

    let Some(watermark_at) = watermark else {
        anyhow::bail!(
            "task_usage_hourly_rollup_state row is missing or watermark is NULL; manual intervention required before migration 103"
        );
    };

    let lag = (max_event - watermark_at).num_seconds();
    if lag <= MAX_LAG_THRESHOLD_SECS {
        tracing::info!(
            %watermark_at,
            %max_event,
            lag_secs = lag,
            "task_usage hourly rollup hook: watermark already current, skipping backfill"
        );
        stamp_watermark(pool).await?;
        return Ok(());
    }

    tracing::info!(%watermark_at, %max_event, lag_secs = lag, "task_usage hourly rollup hook: backfilling under advisory lock");

    // Step 3: serialise against the cron entry / standalone backfill /
    // scheduler via advisory lock 4246 on a dedicated session-pinned conn.
    let mut lock_conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await?;

    let result = async {
        walk_slices(&mut lock_conn, usage_range.min_event.unwrap(), max_event).await?;
        stamp_watermark_on_conn(&mut lock_conn).await
    }
    .await;

    // Best-effort unlock; session-level locks release when the conn closes.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);

    result?;
    Ok(())
}

/// Runs the standalone operator backfill that seeds all historical
/// `task_usage_hourly` buckets.
///
/// The advisory lock is held on a dedicated session for the complete run,
/// including dry-run range inspection. A successful non-dry run stamps the
/// watermark only after every monthly slice has completed, matching the Go
/// command's resumable/idempotent contract.
pub async fn run_operator(
    pool: &PgPool,
    options: OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    let dry_run = options.dry_run;
    let mut lock_conn = cancellable(cancellation, pool.acquire()).await?;
    cancellable(
        cancellation,
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(ADVISORY_LOCK_KEY)
            .execute(&mut *lock_conn),
    )
    .await?;

    let result = async {
        match operator_locked(pool, &mut *lock_conn, options, cancellation).await? {
            OperatorOutcome::Empty => {
                if dry_run {
                    tracing::info!(
                        "task_usage is empty; dry-run complete; watermark left untouched"
                    );
                } else {
                    stamp_watermark_with_cancellation(pool, cancellation).await?;
                    println!("watermark stamped to now() - 5 minutes");
                    tracing::info!("task_usage is empty; watermark stamped");
                }
            }
            OperatorOutcome::DryRun => {
                tracing::info!("dry-run complete; watermark left untouched");
            }
            OperatorOutcome::Complete { rows_touched } => {
                // Keep this write independent of the cancellation token. If the
                // final slice succeeded, the Go command deliberately stamps the
                // watermark with a fresh background context.
                stamp_watermark(pool).await?;
                println!("watermark stamped to now() - 5 minutes");
                tracing::info!(total_rows_touched = rows_touched, "backfill complete");
            }
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
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
                tracing::warn!(error = %unlock_error, "task_usage hourly backfill: advisory unlock failed after run error");
            }
            Err(error)
        }
    }
}

async fn operator_locked(
    pool: &PgPool,
    lock_conn: &mut PgConnection,
    options: OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<OperatorOutcome> {
    let (min_ts, max_ts): (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = cancellable(
        cancellation,
        sqlx::query_as("SELECT MIN(created_at), MAX(created_at) FROM task_usage").fetch_one(pool),
    )
    .await?;

    let (Some(min_ts), Some(max_ts)) = (min_ts, max_ts) else {
        return Ok(OperatorOutcome::Empty);
    };

    let mut from = month_floor(min_ts);
    let end = add_month(month_floor(max_ts))?;

    if options.months_back > 0 {
        let months_back = u32::try_from(options.months_back)
            .map_err(|_| anyhow::anyhow!("months-back value is too large"))?;
        let cutoff = month_floor(Utc::now())
            .checked_sub_months(Months::new(months_back))
            .ok_or_else(|| anyhow::anyhow!("months-back value is too large"))?;
        if cutoff > from {
            if !options.force_partial {
                anyhow::bail!(
                    "--months-back={} would skip buckets before {} (oldest available {}) and the watermark would still advance past them; re-run with --force-partial to accept this, or omit --months-back for a full backfill",
                    options.months_back,
                    cutoff.to_rfc3339(),
                    min_ts.to_rfc3339()
                );
            }
            from = cutoff;
            tracing::warn!(
                months_back = options.months_back,
                effective_from = %from,
                oldest_available = %min_ts,
                "partial backfill: older buckets will be left empty and the watermark will still advance past them"
            );
        }
    }

    tracing::info!(
        from = %from,
        to = %end,
        dry_run = options.dry_run,
        sleep_between_slices = ?options.sleep_between_slices,
        "backfill range"
    );

    let mut cursor = from;
    let mut total_rows = 0_i64;
    while cursor < end {
        if cancellation.is_cancelled() {
            anyhow::bail!("execution cancelled");
        }
        let next = add_month(cursor)?;
        if options.dry_run {
            tracing::info!(from = %cursor, to = %next, "would roll up slice");
            cursor = next;
            continue;
        }

        let rows: i64 = cancellable(
            cancellation,
            sqlx::query_scalar(
                "SELECT rollup_task_usage_hourly_window($1::timestamptz, $2::timestamptz)",
            )
            .bind(cursor)
            .bind(next)
            .fetch_one(&mut *lock_conn),
        )
        .await
        .map_err(|error| anyhow::anyhow!("rollup slice {cursor}..{next}: {error}"))?;
        total_rows += rows;
        tracing::info!(from = %cursor, to = %next, rows_touched = rows, "rolled up slice");
        cursor = next;

        if options.sleep_between_slices > Duration::ZERO && cursor < end {
            tokio::select! {
                _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
                _ = tokio::time::sleep(options.sleep_between_slices) => {},
            }
        }
    }

    if options.dry_run {
        Ok(OperatorOutcome::DryRun)
    } else {
        Ok(OperatorOutcome::Complete {
            rows_touched: total_rows,
        })
    }
}

async fn cancellable<T, F>(cancellation: &CancellationToken, future: F) -> anyhow::Result<T>
where
    F: Future<Output = Result<T, sqlx::Error>>,
{
    tokio::select! {
        _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
        result = future => Ok(result?),
    }
}

async fn walk_slices(
    conn: &mut PgConnection,
    min_event: DateTime<Utc>,
    max_event: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut cursor = month_floor(min_event);
    let end = add_month(month_floor(max_event))?;

    let mut slices_processed: u32 = 0;
    let mut rows_touched: i64 = 0;
    while cursor < end {
        let next = add_month(cursor)?;
        let rows: i64 = sqlx::query_scalar(
            "SELECT rollup_task_usage_hourly_window($1::timestamptz, $2::timestamptz)",
        )
        .bind(cursor)
        .bind(next)
        .fetch_one(&mut *conn)
        .await?;
        slices_processed += 1;
        rows_touched += rows;
        tracing::info!(from = %cursor, to = %next, rows_touched = rows, "task_usage hourly rollup hook: slice complete");
        cursor = next;
    }
    tracing::info!(
        slices = slices_processed,
        total_rows_touched = rows_touched,
        watermark_source = "db_now",
        "task_usage hourly rollup hook: complete"
    );
    Ok(())
}

/// Moves the watermark to `now() - 5 min` using PostgreSQL's clock, matching
/// the cron entry's upper bound and preventing clock-drift stamps into the
/// DB's future.
async fn stamp_watermark(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE task_usage_hourly_rollup_state
           SET watermark_at = now() - INTERVAL '5 minutes'
         WHERE id = 1
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn stamp_watermark_with_cancellation(
    pool: &PgPool,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
        result = stamp_watermark(pool) => result,
    }
}

async fn stamp_watermark_on_conn(conn: &mut PgConnection) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE task_usage_hourly_rollup_state
           SET watermark_at = now() - INTERVAL '5 minutes'
         WHERE id = 1
        "#,
    )
    .execute(conn)
    .await?;
    Ok(())
}

fn month_floor(t: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(t.year(), t.month(), 1, 0, 0, 0)
        .unwrap()
}

fn add_month(t: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
    t.checked_add_months(Months::new(1))
        .ok_or_else(|| anyhow::anyhow!("month overflow at {t}"))
}
