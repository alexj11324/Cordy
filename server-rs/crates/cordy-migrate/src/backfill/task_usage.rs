//! Seeds `task_usage_hourly` from historical `task_usage` rows before
//! migration 103's fail-closed watermark guard (MUL-2957).
//!
//! Port of `server/internal/taskusagebackfill/backfill.go`. The hook does NOT
//! fail when task_usage is empty (fresh DB — the watermark is stamped so
//! migration 103 accepts the empty state) or when the rollup state tables are
//! missing (migrations 101/102 not yet applied). It DOES fail when the rollup
//! walk itself errors, aborting the migrate run.

use anyhow::Context as _;
use chrono::{DateTime, Datelike, Months, SecondsFormat, TimeZone, Utc};
use sqlx::{PgConnection, PgPool};
use std::time::Duration;

/// Shared with rollup_task_usage_hourly(), the standalone backfill command,
/// and the in-process scheduler so a mixed-version cluster cannot double-write.
const ADVISORY_LOCK_KEY: i64 = 4246;

/// Mirrors migration 103's v_lag interval. Below this threshold the migration
/// would have passed anyway, so we save the scan.
const MAX_LAG_THRESHOLD_SECS: i64 = 3600;

struct UsageRange {
    min_event: Option<DateTime<Utc>>,
    max_event: Option<DateTime<Utc>>,
}

pub async fn hook(pool: &PgPool) -> anyhow::Result<()> {
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

    let result = walk_slices(&mut lock_conn, usage_range.min_event.unwrap(), max_event).await;

    // Best-effort unlock; session-level locks release when the conn closes.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);

    result?;
    stamp_watermark(pool).await?;
    Ok(())
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
        let rows = rollup_slice(conn, cursor, next).await?;
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

/// Operator-facing options for the standalone historical backfill.
#[derive(Debug, Clone, Copy, Default)]
pub struct StandaloneOptions {
    pub dry_run: bool,
    pub months_back: i64,
    pub force_partial: bool,
    pub sleep_between_slices: Duration,
}

/// Runs the standalone task-usage backfill while holding the shared session
/// advisory lock. The migration hook and this command intentionally use the
/// same monthly rollup primitive and watermark update.
pub async fn run_standalone(
    pool: &PgPool,
    options: StandaloneOptions,
    shutdown: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    if options.months_back < 0 {
        anyhow::bail!("--months-back must be non-negative");
    }

    let mut lock_conn = pool
        .acquire()
        .await
        .context("acquire advisory-lock connection")?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await
        .context("acquire advisory lock 4246")?;

    let result = run_standalone_locked(pool, &mut *lock_conn, options, shutdown).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await
        .context("release advisory lock 4246");

    result?;
    unlock_result?;
    Ok(())
}

async fn run_standalone_locked(
    pool: &PgPool,
    conn: &mut PgConnection,
    options: StandaloneOptions,
    shutdown: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let (min_ts, max_ts): (Option<DateTime<Utc>>, Option<DateTime<Utc>>) =
        sqlx::query_as(
            "SELECT MIN(created_at), MAX(COALESCE(updated_at, created_at)) FROM task_usage",
        )
            .fetch_one(pool)
            .await
            .context("scan task_usage time range")?;

    let Some(min_ts) = min_ts else {
        tracing::info!("task_usage is empty; nothing to backfill");
        if options.dry_run {
            return Ok(());
        }
        stamp_and_report(pool).await?;
        return Ok(());
    };
    let max_ts = max_ts.context("task_usage has a minimum timestamp but no maximum")?;

    let mut from = month_floor(min_ts);
    let end = add_month(month_floor(max_ts))?;

    if options.months_back > 0 {
        let months_back =
            u32::try_from(options.months_back).context("--months-back is too large")?;
        let cutoff = month_floor(Utc::now())
            .checked_sub_months(Months::new(months_back))
            .context("--months-back underflows the supported date range")?;
        if cutoff > from {
            if !options.force_partial {
                anyhow::bail!(
                    "--months-back={} would skip buckets before {} (oldest available {}) and the watermark would still advance past them; re-run with --force-partial to accept this, or omit --months-back for a full backfill",
                    options.months_back,
                    format_timestamp(cutoff),
                    format_timestamp(min_ts),
                );
            }
            tracing::warn!(
                months_back = options.months_back,
                effective_from = %format_timestamp(cutoff),
                oldest_available = %format_timestamp(min_ts),
                "partial backfill: --months-back limits coverage; older buckets will be left empty and the watermark will still advance past them"
            );
            from = cutoff;
        }
    }

    tracing::info!(
        from = %format_timestamp(from),
        to = %format_timestamp(end),
        dry_run = options.dry_run,
        sleep_between_slices = %format_duration(options.sleep_between_slices),
        "backfill range"
    );

    let mut cursor = from;
    let mut total_rows: i64 = 0;
    while cursor < end {
        if shutdown.is_cancelled() {
            anyhow::bail!("backfill interrupted by signal");
        }
        let next = add_month(cursor)?;
        if options.dry_run {
            tracing::info!(
                from = %format_timestamp(cursor),
                to = %format_timestamp(next),
                "would roll up slice"
            );
            cursor = next;
            continue;
        }

        let rows = rollup_slice(conn, cursor, next).await.with_context(|| {
            format!(
                "rollup slice {}..{}",
                format_timestamp(cursor),
                format_timestamp(next)
            )
        })?;
        total_rows += rows;
        tracing::info!(
            from = %format_timestamp(cursor),
            to = %format_timestamp(next),
            rows_touched = rows,
            "rolled up slice"
        );
        cursor = next;
        if !options.sleep_between_slices.is_zero() && cursor < end {
            tokio::select! {
                () = tokio::time::sleep(options.sleep_between_slices) => {}
                () = shutdown.cancelled() => anyhow::bail!("backfill interrupted by signal"),
            }
        }
    }

    if options.dry_run {
        tracing::info!("dry-run complete; watermark left untouched");
        return Ok(());
    }

    // Once the slice walk is complete, do not observe cancellation until the
    // final watermark write finishes. Leaving all buckets updated with the old
    // watermark makes the next migration/backfill repeat completed work.
    stamp_and_report(pool).await?;
    tracing::info!(total_rows_touched = total_rows, "backfill complete");
    Ok(())
}

async fn rollup_slice(
    conn: &mut PgConnection,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> anyhow::Result<i64> {
    sqlx::query_scalar("SELECT rollup_task_usage_hourly_window($1::timestamptz, $2::timestamptz)")
        .bind(from)
        .bind(to)
        .fetch_one(&mut *conn)
        .await
        .context("execute task_usage hourly rollup window")
}

async fn stamp_and_report(pool: &PgPool) -> anyhow::Result<()> {
    let rows_affected = stamp_watermark(pool).await?;
    if rows_affected == 0 {
        tracing::warn!(
            "no rollup state row to stamp; was the task_usage_hourly schema migration applied?"
        );
    }
    println!("watermark stamped to now() - 5 minutes");
    Ok(())
}

/// Moves the watermark to `now() - 5 min` using PostgreSQL's clock, matching
/// the cron entry's upper bound and preventing clock-drift stamps into the
/// DB's future.
async fn stamp_watermark(pool: &PgPool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE task_usage_hourly_rollup_state
           SET watermark_at = now() - INTERVAL '5 minutes'
         WHERE id = 1
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

fn format_timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn format_duration(duration: Duration) -> String {
    if duration.is_zero() {
        return "0s".to_string();
    }
    format!("{}.{:09}s", duration.as_secs(), duration.subsec_nanos())
}

fn month_floor(t: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(t.year(), t.month(), 1, 0, 0, 0)
        .unwrap()
}

fn add_month(t: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
    t.checked_add_months(Months::new(1))
        .ok_or_else(|| anyhow::anyhow!("month overflow at {t}"))
}
