//! Reconstructs `issue.last_activity_at` from the historical `updated_at`
//! value after every issue writer has been upgraded.
//!
//! Port of `server/internal/issueactivitybackfill` and the standalone
//! `backfill_issue_last_activity` command. Each batch is one independent
//! statement/transaction, so an interrupted run resumes from the remaining
//! NULL rows without holding a transaction over the full table walk.

use std::time::Duration;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

const ADVISORY_LOCK_NAME: &str = "issue_last_activity_backfill";
const DEFAULT_BATCH_SIZE: i64 = 1000;
const DEFAULT_MAX_STALLED_PASSES: i64 = 10;

#[derive(Debug, Clone)]
pub struct OperatorOptions {
    pub batch_size: i64,
    pub sleep_between_batches: Duration,
    pub max_batches: i64,
    pub max_stalled_passes: i64,
}

impl Default for OperatorOptions {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            sleep_between_batches: Duration::from_millis(100),
            max_batches: 0,
            max_stalled_passes: DEFAULT_MAX_STALLED_PASSES,
        }
    }
}

struct BatchResult {
    rows: i64,
    last_id: String,
}

struct StalledPassGuard {
    max: i64,
    consecutive: i64,
}

impl StalledPassGuard {
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

/// Runs the resumable operator backfill under a session advisory lock.
pub async fn run_operator(
    pool: &PgPool,
    options: OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    validate_options(&options)?;
    let mut lock_conn = cancellable(cancellation, pool.acquire()).await?;
    cancellable(
        cancellation,
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(ADVISORY_LOCK_NAME)
            .execute(&mut *lock_conn),
    )
    .await?;

    let result = run_locked(pool, &options, cancellation).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(ADVISORY_LOCK_NAME)
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
                tracing::warn!(error = %unlock_error, "issue last-activity backfill: advisory unlock failed after run error");
            }
            Err(error)
        }
    }
}

async fn run_locked(
    pool: &PgPool,
    options: &OperatorOptions,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    let mut remaining = cancellable(cancellation, count_remaining(pool)).await?;
    tracing::info!(
        remaining,
        batch_size = options.batch_size,
        delay = ?options.sleep_between_batches,
        "issue last-activity backfill started"
    );

    let mut total = 0_i64;
    let mut pass_rows = 0_i64;
    let mut after_id: Option<String> = None;
    let mut pass = 1_i64;
    let mut stalled = StalledPassGuard {
        max: options.max_stalled_passes,
        consecutive: 0,
    };
    let mut batch_number = 1_i64;

    while options.max_batches == 0 || batch_number <= options.max_batches {
        let result = cancellable(
            cancellation,
            run_batch(pool, options.batch_size, after_id.as_deref()),
        )
        .await?;
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
                batch = batch_number,
                pass,
                rows = result.rows,
                total,
                last_id = %result.last_id,
                "issue last-activity batch committed"
            );
            after_id = Some(result.last_id);
        }

        // A short/empty batch ends this keyset pass, but SKIP LOCKED may have
        // left hot rows below the watermark. Count them and wrap to recover.
        if result.rows < options.batch_size {
            remaining = cancellable(cancellation, count_remaining(pool)).await?;
            if remaining == 0 {
                tracing::info!(
                    rows_backfilled = total,
                    remaining = 0,
                    "issue last-activity backfill complete"
                );
                return Ok(());
            }
            stalled.observe(pass_rows, remaining)?;
            tracing::info!(
                pass = pass,
                rows = pass_rows,
                remaining,
                stalled_passes = stalled.consecutive,
                "issue last-activity pass complete; rows remain locked or pending"
            );
            pass += 1;
            after_id = None;
            pass_rows = 0;
        }

        if options.sleep_between_batches > Duration::ZERO {
            tokio::select! {
                _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
                _ = tokio::time::sleep(options.sleep_between_batches) => {},
            }
        }
        batch_number += 1;
    }

    remaining = cancellable(cancellation, count_remaining(pool)).await?;
    tracing::info!(
        rows_backfilled = total,
        remaining,
        "issue last-activity backfill stopped at max batches"
    );
    Ok(())
}

async fn run_batch(
    pool: &PgPool,
    batch_size: i64,
    after_id: Option<&str>,
) -> Result<BatchResult, sqlx::Error> {
    let row: (i64, String) = sqlx::query_as(
        r#"
WITH batch AS (
    SELECT id
    FROM issue
    WHERE last_activity_at IS NULL
      AND ($2::uuid IS NULL OR id > $2::uuid)
    ORDER BY id
    LIMIT $1
    FOR UPDATE SKIP LOCKED
), updated AS (
    UPDATE issue i
    SET last_activity_at = i.updated_at
    FROM batch
    WHERE i.id = batch.id
      AND i.last_activity_at IS NULL
    RETURNING i.id
)
SELECT COUNT(*)::bigint,
       COALESCE((SELECT id::text FROM updated ORDER BY id DESC LIMIT 1), '')
FROM updated
        "#,
    )
    .bind(batch_size)
    .bind(after_id)
    .fetch_one(pool)
    .await?;
    Ok(BatchResult {
        rows: row.0,
        last_id: row.1,
    })
}

async fn count_remaining(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM issue WHERE last_activity_at IS NULL")
        .fetch_one(pool)
        .await
}

fn validate_options(options: &OperatorOptions) -> anyhow::Result<()> {
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

async fn cancellable<T, F>(cancellation: &CancellationToken, future: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    tokio::select! {
        _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
        result = future => Ok(result?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_pass_guard_resets_after_progress_and_fails_at_limit() {
        let mut guard = StalledPassGuard {
            max: 2,
            consecutive: 0,
        };
        assert!(guard.observe(0, 7).is_ok());
        assert!(guard.observe(3, 4).is_ok());
        assert_eq!(guard.consecutive, 0);
        assert!(guard.observe(0, 4).is_ok());
        let error = guard
            .observe(0, 4)
            .expect_err("second stalled pass should fail");
        assert!(error.to_string().contains("2 consecutive passes"));
        assert!(error.to_string().contains("4 rows remaining"));
    }

    #[test]
    fn stalled_pass_guard_zero_disables_the_limit() {
        let mut guard = StalledPassGuard {
            max: 0,
            consecutive: 0,
        };
        for _ in 0..100 {
            assert!(guard.observe(0, 1).is_ok());
        }
    }

    #[test]
    fn options_reject_invalid_bounds() {
        assert!(validate_options(&OperatorOptions {
            batch_size: 0,
            ..OperatorOptions::default()
        })
        .is_err());
        assert!(validate_options(&OperatorOptions {
            max_batches: -1,
            ..OperatorOptions::default()
        })
        .is_err());
        assert!(validate_options(&OperatorOptions {
            max_stalled_passes: -1,
            ..OperatorOptions::default()
        })
        .is_err());
    }
}
