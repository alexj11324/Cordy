use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use patchbay_db::dbid::new_v7;
use sqlx::Row as _;
use uuid::Uuid;

use crate::spec::{HandlerResult, JobSpec, LatestPlanInfo, Scope};
use crate::LeaseLost;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Claim {
    pub id: Uuid,
    pub lease_token: Uuid,
    pub attempt: i32,
    pub kind: ClaimKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimKind {
    Fresh,
    Reclaimed,
    Conflicted,
}

pub(crate) async fn db_now(pool: &sqlx::PgPool) -> anyhow::Result<DateTime<Utc>> {
    sqlx::query_scalar::<_, DateTime<Utc>>("SELECT now()")
        .fetch_one(pool)
        .await
        .context("scheduler: read db now")
}

pub(crate) async fn try_claim(
    pool: &sqlx::PgPool,
    job: &JobSpec,
    scope: &Scope,
    plan_time: DateTime<Utc>,
    db_time: DateTime<Utc>,
    runner_id: &str,
) -> anyhow::Result<Claim> {
    let stale_seconds = duration_seconds(job.stale_timeout);
    let inserted = sqlx::query(
        r#"INSERT INTO sys_cron_executions (
            job_name, scope_kind, scope_id, plan_time,
            status, attempt, max_attempts,
            runner_id, lease_token,
            heartbeat_at, stale_after,
            started_at, updated_at, id
        ) VALUES (
            $1, $2, $3, $4,
            'RUNNING', 1, $5,
            $6, gen_random_uuid(),
            $7::timestamptz, $7::timestamptz + make_interval(secs => $8),
            $7, $7, $9
        )
        ON CONFLICT ON CONSTRAINT uq_sys_cron_execution DO NOTHING
        RETURNING id, lease_token, attempt"#,
    )
    .bind(&job.name)
    .bind(&scope.kind)
    .bind(&scope.id)
    .bind(plan_time)
    .bind(job.max_attempts)
    .bind(runner_id)
    .bind(db_time)
    .bind(stale_seconds)
    .bind(new_v7())
    .fetch_optional(pool)
    .await
    .context("scheduler: claim insert")?;
    if let Some(row) = inserted {
        return Ok(Claim {
            id: row.try_get(0)?,
            lease_token: row.try_get(1)?,
            attempt: row.try_get(2)?,
            kind: ClaimKind::Fresh,
        });
    }

    let reclaimed = sqlx::query(
        r#"UPDATE sys_cron_executions
        SET status = 'RUNNING',
            attempt = attempt + 1,
            runner_id = $1,
            lease_token = gen_random_uuid(),
            heartbeat_at = $2::timestamptz,
            stale_after = $2::timestamptz + make_interval(secs => $3),
            started_at = $2::timestamptz,
            finished_at = NULL,
            duration_ms = NULL,
            next_retry_at = NULL,
            error_code = NULL,
            error_msg = NULL,
            updated_at = $2
        WHERE job_name = $4
          AND scope_kind = $5
          AND scope_id = $6
          AND plan_time = $7
          AND attempt < max_attempts
          AND (
              (status = 'FAILED' AND COALESCE(next_retry_at, $2) <= $2)
              OR (status = 'RUNNING' AND stale_after < $2 AND $8)
          )
        RETURNING id, lease_token, attempt"#,
    )
    .bind(runner_id)
    .bind(db_time)
    .bind(stale_seconds)
    .bind(&job.name)
    .bind(&scope.kind)
    .bind(&scope.id)
    .bind(plan_time)
    .bind(job.allow_stale_reentry)
    .fetch_optional(pool)
    .await
    .context("scheduler: claim steal or retry")?;
    if let Some(row) = reclaimed {
        return Ok(Claim {
            id: row.try_get(0)?,
            lease_token: row.try_get(1)?,
            attempt: row.try_get(2)?,
            kind: ClaimKind::Reclaimed,
        });
    }
    Ok(Claim {
        id: Uuid::nil(),
        lease_token: Uuid::nil(),
        attempt: 0,
        kind: ClaimKind::Conflicted,
    })
}

pub(crate) async fn mark_stale_failed(
    pool: &sqlx::PgPool,
    job_name: &str,
    db_time: DateTime<Utc>,
) -> anyhow::Result<u64> {
    sqlx::query(
        r#"UPDATE sys_cron_executions
        SET status = 'FAILED',
            finished_at = $2,
            error_code = 'stale_timeout',
            error_msg = 'lease expired without heartbeat',
            updated_at = $2
        WHERE job_name = $1
          AND status = 'RUNNING'
          AND stale_after < $2"#,
    )
    .bind(job_name)
    .bind(db_time)
    .execute(pool)
    .await
    .map(|result| result.rows_affected())
    .context("scheduler: mark stale failed")
}

pub(crate) async fn heartbeat(
    pool: &sqlx::PgPool,
    id: Uuid,
    lease_token: Uuid,
    stale_timeout: Duration,
) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"UPDATE sys_cron_executions
        SET heartbeat_at = now(),
            stale_after = now() + make_interval(secs => $3),
            updated_at = now()
        WHERE id = $1
          AND lease_token = $2
          AND status = 'RUNNING'"#,
    )
    .bind(id)
    .bind(lease_token)
    .bind(duration_seconds(stale_timeout))
    .execute(pool)
    .await
    .context("scheduler: heartbeat")?;
    if result.rows_affected() == 0 {
        return Err(LeaseLost.into());
    }
    Ok(())
}

pub(crate) async fn finish_success(
    pool: &sqlx::PgPool,
    claim: Claim,
    db_time: DateTime<Utc>,
    duration_ms: i32,
    result: HandlerResult,
) -> anyhow::Result<()> {
    let result_value = serde_json::Value::Object(result.result);
    let encoded = serde_json::to_vec(&result_value).context("scheduler: marshal result")?;
    anyhow::ensure!(
        encoded.len() <= 16 * 1024,
        "scheduler: result payload too large ({} bytes); keep it small or use logs",
        encoded.len()
    );
    let update = sqlx::query(
        r#"UPDATE sys_cron_executions
        SET status = 'SUCCESS',
            finished_at = $3,
            duration_ms = $4,
            rows_affected = $5,
            result = $6,
            error_code = NULL,
            error_msg = NULL,
            updated_at = $3
        WHERE id = $1
          AND lease_token = $2
          AND status = 'RUNNING'"#,
    )
    .bind(claim.id)
    .bind(claim.lease_token)
    .bind(db_time)
    .bind(duration_ms)
    .bind(result.rows_affected)
    .bind(result_value)
    .execute(pool)
    .await
    .context("scheduler: finish success")?;
    if update.rows_affected() == 0 {
        return Err(LeaseLost.into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn finish_failure(
    pool: &sqlx::PgPool,
    claim: Claim,
    db_time: DateTime<Utc>,
    duration_ms: i32,
    error_code: &str,
    error_message: &str,
    next_retry_at: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    let error_code = if error_code.is_empty() {
        "handler_error"
    } else {
        error_code
    };
    let error_message = truncate_utf8(error_message, 4_000);
    let update = sqlx::query(
        r#"UPDATE sys_cron_executions
        SET status = 'FAILED',
            finished_at = $3,
            duration_ms = $4,
            next_retry_at = $5,
            error_code = $6,
            error_msg = $7,
            updated_at = $3
        WHERE id = $1
          AND lease_token = $2
          AND status = 'RUNNING'"#,
    )
    .bind(claim.id)
    .bind(claim.lease_token)
    .bind(db_time)
    .bind(duration_ms)
    .bind(next_retry_at)
    .bind(error_code)
    .bind(error_message)
    .execute(pool)
    .await
    .context("scheduler: finish failure")?;
    if update.rows_affected() == 0 {
        return Err(LeaseLost.into());
    }
    Ok(())
}

pub(crate) async fn latest_plan(
    pool: &sqlx::PgPool,
    job_name: &str,
    scope: &Scope,
) -> anyhow::Result<LatestPlanInfo> {
    let row = sqlx::query(
        r#"SELECT plan_time, status, attempt, max_attempts, next_retry_at
        FROM sys_cron_executions
        WHERE job_name = $1 AND scope_kind = $2 AND scope_id = $3
        ORDER BY plan_time DESC
        LIMIT 1"#,
    )
    .bind(job_name)
    .bind(&scope.kind)
    .bind(&scope.id)
    .fetch_optional(pool)
    .await
    .context("scheduler: read latest plan")?;
    let Some(row) = row else {
        return Ok(LatestPlanInfo::default());
    };
    Ok(LatestPlanInfo {
        found: true,
        plan_time: Some(row.try_get(0)?),
        status: row.try_get(1)?,
        attempt: row.try_get(2)?,
        max_attempts: row.try_get(3)?,
        next_retry_at: row.try_get(4)?,
    })
}

fn duration_seconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX).max(1)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_message_cap_preserves_utf8() {
        let value = format!("{}界", "a".repeat(3_999));
        let truncated = truncate_utf8(&value, 4_000);
        assert_eq!(truncated.len(), 3_999);
        assert!(truncated.is_char_boundary(truncated.len()));
    }
}
