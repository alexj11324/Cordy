//! Bounded, resumable reconstruction of issue.last_activity_at.
//!
//! Port of server/internal/issueactivitybackfill plus the
//! server/cmd/backfill_issue_last_activity operator entrypoint. The walk is
//! intentionally separate from migrations and startup so a large issue table
//! never blocks a deploy.

use std::time::Duration;

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

pub const DEFAULT_BATCH_SIZE: i64 = 1_000;
pub const DEFAULT_SLEEP_BETWEEN_BATCHES: Duration = Duration::from_millis(100);
pub const DEFAULT_MAX_STALLED_PASSES: u32 = 10;

const ADVISORY_LOCK_NAME: &str = "issue_last_activity_backfill";

const BATCH_SQL: &str = r#"
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
       (SELECT id FROM updated ORDER BY id DESC LIMIT 1)
FROM updated
"#;

const COUNT_REMAINING_SQL: &str = r#"
SELECT count(*)::bigint
FROM issue
WHERE last_activity_at IS NULL
"#;

/// Operator controls for the backfill walk.
#[derive(Clone, Debug)]
pub struct Options {
    pub batch_size: i64,
    pub sleep_between_batches: Duration,
    /// None means finish all rows. Some(0) is normalized to None.
    pub max_batches: Option<u64>,
    /// Zero disables the no-progress guard.
    pub max_stalled_passes: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            sleep_between_batches: DEFAULT_SLEEP_BETWEEN_BATCHES,
            max_batches: None,
            max_stalled_passes: DEFAULT_MAX_STALLED_PASSES,
        }
    }
}

impl Options {
    /// Parses the operator flags accepted by the Go command.
    pub fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut options = Self::default();
        let mut args = args.peekable();

        while let Some(argument) = args.next() {
            if argument == "--help" || argument == "-h" {
                anyhow::bail!(
                    "usage: cordy-migrate backfill-issue-last-activity [--batch-size N] [--sleep-between-batches DURATION] [--max-batches N] [--max-stalled-passes N]"
                );
            }

            let (name, value) = if let Some((name, value)) = argument.split_once('=') {
                (name.to_string(), value.to_string())
            } else {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for {argument}"))?;
                (argument, value)
            };

            match name.as_str() {
                "--batch-size" => {
                    options.batch_size = value
                        .parse()
                        .map_err(|error| anyhow::anyhow!("invalid --batch-size {value:?}: {error}"))?;
                }
                "--sleep-between-batches" => {
                    options.sleep_between_batches = parse_duration(&value)?;
                }
                "--max-batches" => {
                    let value: u64 = value
                        .parse()
                        .map_err(|error| anyhow::anyhow!("invalid --max-batches {value:?}: {error}"))?;
                    options.max_batches = (value > 0).then_some(value);
                }
                "--max-stalled-passes" => {
                    options.max_stalled_passes = value.parse().map_err(|error| {
                        anyhow::anyhow!("invalid --max-stalled-passes {value:?}: {error}")
                    })?;
                }
                _ => anyhow::bail!(
                    "unknown option {name}; usage: cordy-migrate backfill-issue-last-activity [--batch-size N] [--sleep-between-batches DURATION] [--max-batches N] [--max-stalled-passes N]"
                ),
            }
        }

        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.batch_size < 1 {
            anyhow::bail!("--batch-size must be at least 1");
        }
        Ok(())
    }
}

/// Result of one SQL batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchResult {
    pub rows: i64,
    pub last_id: Option<Uuid>,
}

/// Summary emitted after a complete or bounded run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Summary {
    pub rows_backfilled: i64,
    pub remaining: i64,
    pub batches: u64,
    pub passes: u64,
}

/// Executes one bounded batch. Each call is a separate autocommit statement.
pub async fn batch(
    conn: &mut PgConnection,
    batch_size: i64,
    after_id: Option<Uuid>,
) -> anyhow::Result<BatchResult> {
    if batch_size < 1 {
        anyhow::bail!("batch size must be at least 1");
    }
    let (rows, last_id): (i64, Option<Uuid>) = sqlx::query_as(BATCH_SQL)
        .bind(batch_size)
        .bind(after_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|error| anyhow::anyhow!("backfill issue last_activity_at batch: {error}"))?;
    Ok(BatchResult { rows, last_id })
}

/// Counts rows that still need reconstruction.
pub async fn count_remaining(conn: &mut PgConnection) -> anyhow::Result<i64> {
    sqlx::query_scalar(COUNT_REMAINING_SQL)
        .fetch_one(&mut *conn)
        .await
        .map_err(|error| anyhow::anyhow!("count issue last_activity_at backlog: {error}"))
}

/// Runs the operator walk under a session advisory lock.
///
/// The same pinned connection is used for the lock, count, and update
/// statements. That keeps the session-level lock meaningful across the whole
/// walk while each batch remains independently committed.
pub async fn run(pool: &PgPool, options: Options) -> anyhow::Result<Summary> {
    options.validate()?;

    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(ADVISORY_LOCK_NAME)
        .execute(&mut *conn)
        .await?;

    let result = run_locked(&mut conn, &options).await;
    let unlock = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(ADVISORY_LOCK_NAME)
        .execute(&mut *conn)
        .await;

    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(anyhow::anyhow!("release advisory lock: {error}")),
        (Ok(summary), Ok(_)) => Ok(summary),
    }
}

async fn run_locked(conn: &mut PgConnection, options: &Options) -> anyhow::Result<Summary> {
    let starting_remaining = count_remaining(conn).await?;
    tracing::info!(
        remaining = starting_remaining,
        batch_size = options.batch_size,
        delay_ms = options.sleep_between_batches.as_millis() as u64,
        "issue last-activity backfill started"
    );

    let mut total = 0_i64;
    let mut after_id = None;
    let mut pass_rows = 0_i64;
    let mut pass = 1_u64;
    let mut stalled_passes = 0_u32;
    let mut batches = 0_u64;

    loop {
        if options
            .max_batches
            .is_some_and(|max_batches| batches >= max_batches)
        {
            break;
        }

        let result = batch(conn, options.batch_size, after_id).await?;
        batches += 1;

        total += result.rows;
        pass_rows += result.rows;
        if result.rows > 0 {
            let Some(last_id) = result.last_id else {
                anyhow::bail!(
                    "issue last-activity backfill batch returned {} rows without a keyset watermark",
                    result.rows
                );
            };
            tracing::info!(
                batch = batches,
                pass,
                rows = result.rows,
                total,
                last_id = %last_id,
                "issue last-activity batch committed"
            );
            after_id = Some(last_id);
        }

        // A short or empty batch ends this keyset pass. SKIP LOCKED may have
        // skipped hot rows below the watermark, so a later pass wraps around.
        if result.rows < options.batch_size {
            let current_remaining = count_remaining(conn).await?;
            if current_remaining == 0 {
                tracing::info!(
                    rows_backfilled = total,
                    remaining = current_remaining,
                    "issue last-activity backfill complete"
                );
                return Ok(Summary {
                    rows_backfilled: total,
                    remaining: current_remaining,
                    batches,
                    passes: pass,
                });
            }

            if pass_rows == 0 {
                stalled_passes += 1;
            } else {
                stalled_passes = 0;
            }
            if options.max_stalled_passes > 0 && stalled_passes >= options.max_stalled_passes {
                anyhow::bail!(
                    "issue last-activity backfill stalled: {} consecutive passes made no progress with {} rows remaining; release long-held row locks and rerun, or increase --max-stalled-passes",
                    stalled_passes,
                    current_remaining
                );
            }

            tracing::info!(
                pass,
                rows = pass_rows,
                remaining = current_remaining,
                stalled_passes,
                "issue last-activity pass complete; rows remain locked or pending"
            );
            pass += 1;
            after_id = None;
            pass_rows = 0;
        }

        if !options.sleep_between_batches.is_zero() {
            tokio::time::sleep(options.sleep_between_batches).await;
        }
    }

    let remaining = count_remaining(conn).await?;
    tracing::info!(
        rows_backfilled = total,
        remaining,
        batches,
        passes = pass,
        "issue last-activity backfill stopped at max batches"
    );
    Ok(Summary {
        rows_backfilled: total,
        remaining,
        batches,
        passes: pass,
    })
}

fn parse_duration(raw: &str) -> anyhow::Result<Duration> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('-') {
        anyhow::bail!("invalid duration {raw:?}");
    }
    if raw == "0" {
        return Ok(Duration::ZERO);
    }

    let bytes = raw.as_bytes();
    let mut cursor = 0;
    let mut seconds = 0.0_f64;
    while cursor < bytes.len() {
        let number_start = cursor;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b'.') {
            cursor += 1;
        }
        if cursor == number_start {
            anyhow::bail!("invalid duration {raw:?}");
        }
        let value: f64 = raw[number_start..cursor]
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration {raw:?}"))?;

        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| raw[cursor..].starts_with(unit))
        .ok_or_else(|| anyhow::anyhow!("invalid duration {raw:?}"))?;
        cursor += unit.len();
        seconds += value * multiplier;
    }

    if !seconds.is_finite() || seconds < 0.0 || seconds >= Duration::MAX.as_secs_f64() {
        anyhow::bail!("invalid duration {raw:?}");
    }
    Ok(Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_matches_go_operator_defaults() {
        let options = Options::default();
        assert_eq!(options.batch_size, 1_000);
        assert_eq!(options.sleep_between_batches, Duration::from_millis(100));
        assert_eq!(options.max_batches, None);
        assert_eq!(options.max_stalled_passes, 10);
    }

    #[test]
    fn options_parse_and_validate_operator_flags() {
        let options = Options::parse(
            [
                "--batch-size=25",
                "--sleep-between-batches",
                "250ms",
                "--max-batches",
                "3",
                "--max-stalled-passes=4",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(options.batch_size, 25);
        assert_eq!(options.sleep_between_batches, Duration::from_millis(250));
        assert_eq!(options.max_batches, Some(3));
        assert_eq!(options.max_stalled_passes, 4);
    }

    #[test]
    fn options_reject_invalid_values() {
        for args in [
            vec!["--batch-size", "0"],
            vec!["--sleep-between-batches", "-1s"],
            vec!["--max-batches", "-1"],
            vec!["--max-stalled-passes", "-1"],
        ] {
            assert!(Options::parse(args.into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn duration_parser_matches_go_shapes() {
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
        assert_eq!(
            parse_duration("1s500ms").unwrap(),
            Duration::from_millis(1_500)
        );
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert!(parse_duration("not-a-duration").is_err());
    }
}
