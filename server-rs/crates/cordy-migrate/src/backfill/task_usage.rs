//! Seeds `task_usage_hourly` from historical `task_usage` rows before
//! migration 103's fail-closed watermark guard (MUL-2957).
//!
//! Port of `server/internal/taskusagebackfill/backfill.go`. The hook does NOT
//! fail when task_usage is empty (fresh DB — the watermark is stamped so
//! migration 103 accepts the empty state) or when the rollup state tables are
//! missing (migrations 101/102 not yet applied). It DOES fail when the rollup
//! walk itself errors, aborting the migrate run.

use chrono::{DateTime, Datelike, Months, TimeZone, Utc};
use sqlx::{PgConnection, PgPool};

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

    let result = walk_slices(&mut lock_conn, usage_range.min_event.unwrap(), max_event).await;

    // Best-effort unlock; session-level locks release when the conn closes.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_LOCK_KEY)
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);

    stamp_watermark(pool).await?;
    result?;
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

fn month_floor(t: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(t.year(), t.month(), 1, 0, 0, 0)
        .unwrap()
}

fn add_month(t: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
    t.checked_add_months(Months::new(1))
        .ok_or_else(|| anyhow::anyhow!("month overflow at {t}"))
}
