use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::{static_scopes, CatchUpMode, HandlerResult, JobSpec, GLOBAL_SCOPE};

mod autopilot;

pub use autopilot::{
    autopilot_schedule_dispatch_job, AutopilotScheduleDispatcher, AUTOPILOT_SCHEDULE_DISPATCH_JOB,
    AUTOPILOT_TRIGGER_SCOPE, DEFAULT_AUTOPILOT_SCHEDULE_TIMEZONE,
};

/// Stable audit name. Renaming it would detach existing execution history.
pub const TASK_USAGE_HOURLY_JOB: &str = "rollup_task_usage_hourly";

/// Shared by the SQL rollup function and every manual backfill path.
pub const TASK_USAGE_ADVISORY_LOCK_ID: i64 = 4246;

/// Drives the existing task-usage SQL rollup through the durable scheduler.
///
/// The SQL function owns advisory lock 4246. Losing that inner race is a
/// successful no-op, not a retryable scheduler failure.
pub fn task_usage_hourly_job(pool: PgPool) -> JobSpec {
    let handler_pool = pool.clone();
    JobSpec {
        name: TASK_USAGE_HOURLY_JOB.into(),
        cadence: Duration::from_secs(5 * 60),
        schedule_delay: Duration::from_secs(5 * 60),
        catch_up_mode: CatchUpMode::LatestOnly,
        catch_up_window: Duration::from_secs(24 * 60 * 60),
        max_plans_per_tick: 0,
        run_timeout: Duration::from_secs(25 * 60),
        stale_timeout: Duration::from_secs(30 * 60),
        heartbeat_interval: Duration::from_secs(30),
        allow_stale_reentry: true,
        max_attempts: 3,
        retry_backoff: vec![
            Duration::from_secs(60),
            Duration::from_secs(5 * 60),
            Duration::from_secs(15 * 60),
        ],
        scopes: static_scopes([GLOBAL_SCOPE.clone()]),
        plans_for_scope: None,
        handler: Arc::new(move |cancel, input| {
            let pool = handler_pool.clone();
            Box::pin(async move {
                let watermark_before = read_task_usage_watermark(&pool)
                    .await
                    .context("read watermark before")?;

                let rows = sqlx::query_scalar::<_, i64>("SELECT rollup_task_usage_hourly()")
                    .fetch_one(&pool)
                    .await
                    .context("rollup_task_usage_hourly")?;

                let watermark_after = read_task_usage_watermark(&pool)
                    .await
                    .context("read watermark after")?;

                // Match Go: a final best-effort heartbeat keeps short runs fresh.
                let _ = (input.heartbeat)(cancel).await;

                Ok(HandlerResult {
                    rows_affected: rows,
                    result: task_usage_result(watermark_before, watermark_after),
                })
            })
        }),
    }
}

async fn read_task_usage_watermark(pool: &PgPool) -> anyhow::Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar::<_, DateTime<Utc>>(
        "SELECT watermark_at FROM task_usage_hourly_rollup_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("query task_usage_hourly_rollup_state")
}

fn task_usage_result(
    watermark_before: Option<DateTime<Utc>>,
    watermark_after: Option<DateTime<Utc>>,
) -> Map<String, Value> {
    let mut result = Map::from_iter([(
        "advisory_lock_id".into(),
        json!(TASK_USAGE_ADVISORY_LOCK_ID),
    )]);
    if let Some(value) = watermark_before {
        result.insert("watermark_before".into(), json!(rfc3339_seconds(value)));
    }
    if let Some(value) = watermark_after {
        result.insert("watermark_after".into(), json!(rfc3339_seconds(value)));
    }
    result
}

fn rfc3339_seconds(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn task_usage_result_omits_missing_watermarks() {
        assert_eq!(
            task_usage_result(None, None),
            Map::from_iter([("advisory_lock_id".into(), json!(4246))])
        );
    }

    #[test]
    fn task_usage_result_uses_go_timestamp_precision() {
        let watermark = DateTime::parse_from_rfc3339("2026-08-23T12:34:56.789Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        assert_eq!(
            task_usage_result(Some(watermark), None)["watermark_before"],
            json!("2026-08-23T12:34:56Z")
        );
    }
}
