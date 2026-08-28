//! Reconciles `agent_task_queue` rows to the Human Attribution strict
//! invariant BEFORE migration 198 validates it.
//!
//! Port of `server/internal/attributionbackfill/backfill.go` (GH #5544 /
//! MUL-4302 / MUL-4897): migration 197 installs the strict cross-column CHECK
//! as NOT VALID and 198 runs VALIDATE CONSTRAINT. Self-hosted databases that
//! never ran the out-of-band backfill have legacy rows that fail 198 closed;
//! this hook mirrors originator_user_id into accountable_user_id for exactly
//! those rows so VALIDATE passes with zero operator steps.

use sqlx::PgPool;

const DEFAULT_BATCH_SIZE: i64 = 5000;

/// Counts rows where both attribution users are set but disagree — the "real
/// mis-attribution" shape worth a warning. Table name is a trusted constant,
/// never user input.
const COUNT_MISMATCH_SQL: &str = r#"
SELECT count(*)
FROM agent_task_queue
WHERE originator_user_id IS NOT NULL
  AND accountable_user_id IS NOT NULL
  AND accountable_user_id <> originator_user_id"#;

/// Mirrors originator_user_id into accountable_user_id for a bounded batch of
/// violating rows and stamps originator_source='backfill' only where NULL.
///
/// Concurrency safety (defense in depth): FOR UPDATE in the CTE re-evaluates
/// the predicate under READ COMMITTED once the lock is granted, and the outer
/// UPDATE repeats the predicate — a row flipped to a legitimate originator-NULL
/// fork by a concurrent writer is dropped instead of clobbered.
const BACKFILL_BATCH_SQL: &str = r#"
WITH batch AS (
    SELECT id
    FROM agent_task_queue
    WHERE originator_user_id IS NOT NULL
      AND accountable_user_id IS DISTINCT FROM originator_user_id
    LIMIT $1
    FOR UPDATE
)
UPDATE agent_task_queue q
SET accountable_user_id = q.originator_user_id,
    originator_source   = COALESCE(q.originator_source, 'backfill')
FROM batch
WHERE q.id = batch.id
  AND q.originator_user_id IS NOT NULL
  AND q.accountable_user_id IS DISTINCT FROM q.originator_user_id"#;

pub async fn hook(pool: &PgPool) -> anyhow::Result<()> {
    let mismatch_normalized: i64 = sqlx::query_scalar(COUNT_MISMATCH_SQL)
        .fetch_one(pool)
        .await?;
    if mismatch_normalized > 0 {
        tracing::warn!(
            mismatch_rows = mismatch_normalized,
            "attribution backfill: normalizing rows where accountable_user_id disagreed with a non-NULL originator_user_id; originator is authoritative but these are worth auditing"
        );
    }

    let mut rows_backfilled: u64 = 0;
    let mut batches: u32 = 0;
    loop {
        let result = sqlx::query(BACKFILL_BATCH_SQL)
            .bind(DEFAULT_BATCH_SIZE)
            .execute(pool)
            .await?;
        let n = result.rows_affected();
        if n == 0 {
            break;
        }
        rows_backfilled += n;
        batches += 1;
        tracing::info!(
            rows = n,
            total = rows_backfilled,
            "attribution backfill: batch reconciled"
        );
    }

    if rows_backfilled == 0 {
        tracing::info!("attribution backfill: no rows needed reconciliation before migration 198");
    } else {
        tracing::info!(
            rows_backfilled,
            batches,
            mismatch_normalized,
            "attribution backfill: complete"
        );
    }
    Ok(())
}
