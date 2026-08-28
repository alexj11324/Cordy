//! Resumable batches for seeding `issue.last_activity_at` from `updated_at`.
//!
//! Each batch is
//! an independent statement/transaction, uses an id keyset watermark, and
//! skips rows locked by unrelated writers.

use anyhow::Context as _;
use sqlx::PgPool;
use std::time::Duration;

/// Mirrors the Go command's bounded transaction size.
pub const DEFAULT_BATCH_SIZE: i64 = 1_000;

const ADVISORY_LOCK_NAME: &str = "issue_last_activity_backfill";

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub batch_size: i64,
    pub sleep_between_batches: Duration,
    pub max_batches: i64,
    pub max_stalled_passes: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            sleep_between_batches: Duration::from_millis(100),
            max_batches: 0,
            max_stalled_passes: 10,
        }
    }
}

#[derive(Debug)]
struct BatchResult {
    rows: i64,
    last_id: String,
}

/// Runs the operator backfill while holding the session-level lock shared by
/// concurrent operators. Batch statements use the pool, matching the Go
/// command while the dedicated connection keeps the lock for the whole run.
pub async fn run(pool: &PgPool, options: Options) -> anyhow::Result<()> {
    validate_options(options)?;

    let mut lock_conn = pool
        .acquire()
        .await
        .context("acquire advisory-lock connection")?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(ADVISORY_LOCK_NAME)
        .execute(&mut *lock_conn)
        .await
        .context("acquire issue last-activity advisory lock")?;

    let result = run_locked(pool, options).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(ADVISORY_LOCK_NAME)
        .execute(&mut *lock_conn)
        .await
        .context("release issue last-activity advisory lock");

    result?;
    unlock_result?;
    Ok(())
}

pub fn validate_options(options: Options) -> anyhow::Result<()> {
    if options.batch_size < 1 {
        anyhow::bail!("--batch-size must be at least 1");
    }
    if options.max_batches < 0 {
        anyhow::bail!("--max-batches must not be negative");
    }
    if options.max_stalled_passes < 0 {
        anyhow::bail!("--max-stalled-passes must not be negative");
    }
    Ok(())
}

async fn run_locked(pool: &PgPool, options: Options) -> anyhow::Result<()> {
    let mut remaining = count_remaining(pool).await?;
    tracing::info!(
        remaining,
        batch_size = options.batch_size,
        delay = ?options.sleep_between_batches,
        "issue last-activity backfill started"
    );

    let mut total: i64 = 0;
    let mut pass_rows: i64 = 0;
    let mut after_id: Option<String> = None;
    let mut pass: i64 = 1;
    let mut stall_guard = StalledPassGuard::new(options.max_stalled_passes);
    let mut batch: i64 = 1;

    while options.max_batches == 0 || batch <= options.max_batches {
        let result = batch_rows(pool, options.batch_size, after_id.as_deref()).await?;
        if result.rows > 0 && result.last_id.is_empty() {
            anyhow::bail!(
                "issue last-activity backfill batch returned {} rows without a keyset watermark",
                result.rows
            );
        }

        total += result.rows;
        pass_rows += result.rows;
        if result.rows > 0 {
            tracing::info!(
                batch,
                pass,
                rows = result.rows,
                total,
                last_id = %result.last_id,
                "issue last-activity batch committed"
            );
            after_id = Some(result.last_id);
        }

        // A short/empty keyset batch ends this pass, not necessarily the
        // whole walk: SKIP LOCKED can leave hot rows below the watermark.
        if result.rows < options.batch_size {
            remaining = count_remaining(pool).await?;
            if remaining == 0 {
                tracing::info!(
                    rows_backfilled = total,
                    remaining = 0,
                    "issue last-activity backfill complete"
                );
                return Ok(());
            }
            stall_guard.observe(pass_rows, remaining)?;
            tracing::info!(
                completed_pass = pass,
                rows = pass_rows,
                remaining,
                stalled_passes = stall_guard.consecutive,
                "issue last-activity pass complete; rows remain locked or pending"
            );
            pass += 1;
            after_id = None;
            pass_rows = 0;
        }

        if !options.sleep_between_batches.is_zero() {
            tokio::time::sleep(options.sleep_between_batches).await;
        }
        batch += 1;
    }

    remaining = count_remaining(pool).await?;
    tracing::info!(
        rows_backfilled = total,
        remaining,
        "issue last-activity backfill stopped at max batches"
    );
    Ok(())
}

#[derive(Debug)]
struct StalledPassGuard {
    max: i64,
    consecutive: i64,
}

impl StalledPassGuard {
    fn new(max: i64) -> Self {
        Self {
            max,
            consecutive: 0,
        }
    }

    fn observe(&mut self, pass_rows: i64, remaining: i64) -> anyhow::Result<()> {
        if pass_rows > 0 {
            self.consecutive = 0;
            return Ok(());
        }
        self.consecutive += 1;
        if self.max > 0 && self.consecutive >= self.max {
            anyhow::bail!(
                "issue last-activity backfill stalled: {} consecutive passes made no progress with {} rows remaining; release long-held row locks and rerun, or increase --max-stalled-passes",
                self.consecutive,
                remaining
            );
        }
        Ok(())
    }
}

async fn batch_rows(
    pool: &PgPool,
    batch_size: i64,
    after_id: Option<&str>,
) -> anyhow::Result<BatchResult> {
    batch_rows_from_table(pool, batch_size, after_id, "issue").await
}

async fn batch_rows_from_table(
    pool: &PgPool,
    batch_size: i64,
    after_id: Option<&str>,
    table: &str,
) -> anyhow::Result<BatchResult> {
    // `table` is a fixed production identifier; the test-only override uses a
    // generated schema name and never accepts operator input.
    let query = format!(
        r#"
WITH batch AS (
    SELECT id
    FROM {table}
    WHERE last_activity_at IS NULL
      AND ($2::uuid IS NULL OR id > $2::uuid)
    ORDER BY id
    LIMIT $1
    FOR UPDATE SKIP LOCKED
), updated AS (
UPDATE {table} i
SET last_activity_at = i.updated_at
FROM batch
WHERE i.id = batch.id
  AND i.last_activity_at IS NULL
  RETURNING i.id
)
SELECT COUNT(*)::bigint,
       COALESCE((SELECT id::text FROM updated ORDER BY id DESC LIMIT 1), '')
FROM updated"#
    );
    let (rows, last_id): (i64, String) = sqlx::query_as(&query)
        .bind(batch_size)
        .bind(after_id)
        .fetch_one(pool)
        .await
        .context("backfill issue last_activity_at batch")?;
    Ok(BatchResult { rows, last_id })
}

async fn count_remaining(pool: &PgPool) -> anyhow::Result<i64> {
    count_remaining_from_table(pool, "issue").await
}

async fn count_remaining_from_table(pool: &PgPool, table: &str) -> anyhow::Result<i64> {
    let query = format!("SELECT count(*)::bigint FROM {table} WHERE last_activity_at IS NULL");
    sqlx::query_scalar(&query)
        .fetch_one(pool)
        .await
        .context("count issue last_activity_at backlog")
}

#[cfg(test)]
mod tests {
    use super::{batch_rows_from_table, count_remaining_from_table, StalledPassGuard};
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn test_pool() -> Option<PgPool> {
        let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&db_url)
            .await
            .ok()?;
        if sqlx::query("SELECT 1").execute(&pool).await.is_err() {
            pool.close().await;
            return None;
        }
        Some(pool)
    }

    async fn fixture(pool: &PgPool) -> Option<String> {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let schema = format!("issue_activity_backfill_{}_{}", std::process::id(), suffix);
        let create_schema = format!("CREATE SCHEMA \"{schema}\"");
        sqlx::query(&create_schema).execute(pool).await.ok()?;
        let table = format!("\"{schema}\".issue");
        let create_table = format!(
            r#"
CREATE TABLE {table} (
    id UUID PRIMARY KEY,
    updated_at TIMESTAMPTZ NOT NULL,
    last_activity_at TIMESTAMPTZ
)"#
        );
        if sqlx::query(&create_table).execute(pool).await.is_err() {
            let _ = sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
                .execute(pool)
                .await;
            return None;
        }
        Some(table)
    }

    async fn drop_fixture(pool: &PgPool, table: &str) {
        let schema = table
            .strip_prefix('"')
            .and_then(|value| value.split_once('"'))
            .map(|(schema, _)| schema);
        if let Some(schema) = schema {
            let _ = sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
                .execute(pool)
                .await;
        }
    }

    #[test]
    fn stalled_guard_resets_after_progress_and_then_fails() {
        let mut guard = StalledPassGuard::new(2);
        guard.observe(0, 7).unwrap();
        guard.observe(3, 4).unwrap();
        assert_eq!(guard.consecutive, 0);
        guard.observe(0, 4).unwrap();
        let error = guard.observe(0, 4).unwrap_err().to_string();
        assert!(error.contains("2 consecutive passes"));
        assert!(error.contains("4 rows remaining"));
    }

    #[test]
    fn stalled_guard_can_be_disabled() {
        let mut guard = StalledPassGuard::new(0);
        for _ in 0..100 {
            guard.observe(0, 1).unwrap();
        }
    }

    #[tokio::test]
    async fn batch_is_bounded_and_resumable() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping issue activity integration test: database unavailable");
            return;
        };
        let Some(table) = fixture(&pool).await else {
            pool.close().await;
            eprintln!("skipping issue activity integration test: fixture unavailable");
            return;
        };
        let insert = format!(
            "INSERT INTO {table} (id, updated_at, last_activity_at) VALUES ($1::uuid, $2::timestamptz, $3::timestamptz)"
        );
        for (id, updated, activity) in [
            (
                "00000000-0000-0000-0000-000000000001",
                "2026-01-01T00:00:00Z",
                None,
            ),
            (
                "00000000-0000-0000-0000-000000000002",
                "2026-01-02T00:00:00Z",
                None,
            ),
            (
                "00000000-0000-0000-0000-000000000003",
                "2026-01-03T00:00:00Z",
                Some("2026-02-01T00:00:00Z"),
            ),
        ] {
            sqlx::query(&insert)
                .bind(id)
                .bind(updated)
                .bind(activity)
                .execute(&pool)
                .await
                .expect("seed fixture");
        }

        let first = batch_rows_from_table(&pool, 1, None, &table)
            .await
            .expect("first batch");
        assert_eq!(first.rows, 1);
        assert_eq!(first.last_id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(count_remaining_from_table(&pool, &table).await.unwrap(), 1);

        let second = batch_rows_from_table(&pool, 10, Some(&first.last_id), &table)
            .await
            .expect("second batch");
        assert_eq!(second.rows, 1);
        assert_eq!(second.last_id, "00000000-0000-0000-0000-000000000002");

        let idempotent = batch_rows_from_table(&pool, 10, Some(&second.last_id), &table)
            .await
            .expect("idempotent batch");
        assert_eq!(idempotent.rows, 0);
        assert!(idempotent.last_id.is_empty());

        let preserved: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(&format!(
            "SELECT last_activity_at FROM {table} WHERE id = '00000000-0000-0000-0000-000000000003'"
        ))
        .fetch_one(&pool)
        .await
        .expect("read pre-populated row");
        assert_eq!(preserved.to_rfc3339(), "2026-02-01T00:00:00+00:00");
        drop_fixture(&pool, &table).await;
        pool.close().await;
    }

    #[tokio::test]
    async fn batch_wraps_to_recover_rows_skipped_by_lock() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping issue activity integration test: database unavailable");
            return;
        };
        let Some(table) = fixture(&pool).await else {
            pool.close().await;
            eprintln!("skipping issue activity integration test: fixture unavailable");
            return;
        };
        let insert =
            format!("INSERT INTO {table} (id, updated_at) VALUES ($1::uuid, $2::timestamptz)");
        for (id, updated) in [
            (
                "00000000-0000-0000-0000-000000000001",
                "2026-01-01T00:00:00Z",
            ),
            (
                "00000000-0000-0000-0000-000000000002",
                "2026-01-02T00:00:00Z",
            ),
            (
                "00000000-0000-0000-0000-000000000003",
                "2026-01-03T00:00:00Z",
            ),
        ] {
            sqlx::query(&insert)
                .bind(id)
                .bind(updated)
                .execute(&pool)
                .await
                .expect("seed fixture");
        }

        let mut lock_tx = pool.begin().await.expect("begin lock transaction");
        sqlx::query(&format!(
            "SELECT id FROM {table} WHERE id = '00000000-0000-0000-0000-000000000001' FOR UPDATE"
        ))
        .fetch_one(&mut *lock_tx)
        .await
        .expect("lock first row");

        let first = batch_rows_from_table(&pool, 2, None, &table)
            .await
            .expect("locked-row batch");
        assert_eq!(first.rows, 2);
        assert_eq!(first.last_id, "00000000-0000-0000-0000-000000000003");
        lock_tx.rollback().await.expect("release first row");

        let end_of_pass = batch_rows_from_table(&pool, 2, Some(&first.last_id), &table)
            .await
            .expect("end-of-pass batch");
        assert_eq!(end_of_pass.rows, 0);
        assert_eq!(count_remaining_from_table(&pool, &table).await.unwrap(), 1);

        let recovered = batch_rows_from_table(&pool, 2, None, &table)
            .await
            .expect("wrapped batch");
        assert_eq!(recovered.rows, 1);
        assert_eq!(recovered.last_id, "00000000-0000-0000-0000-000000000001");
        drop_fixture(&pool, &table).await;
        pool.close().await;
    }
}
