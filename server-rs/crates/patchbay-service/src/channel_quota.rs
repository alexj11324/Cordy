//! Hosted IM turn admission.
//!
//! Channel messages are already durable in `chat_message` and are linked to
//! their task in the same transaction. This module deliberately uses that
//! server-owned record as the usage ledger instead of a client counter: the
//! workspace row lock serialises concurrent admissions, and a failed enqueue
//! rolls the reservation back with the surrounding transaction. Once a task
//! row commits, its turn remains usage even if the run later fails or is
//! cancelled; only the pre-commit reservation is released by rollback.

use std::env;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

/// The exact entitlement window used for both quota admission and usage
/// reporting.  Cloud owns the boundaries; the service must never reconstruct
/// them from the current calendar month when a policy supplies them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelQuotaWindow {
    pub limit: i64,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub reset_at: DateTime<Utc>,
}

impl ChannelQuotaWindow {
    pub fn current_month(limit: i64, now: DateTime<Utc>) -> Self {
        let period_start = Utc
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .single()
            .expect("the first day of a UTC month is always valid");
        let (next_year, next_month) = if now.month() == 12 {
            (now.year() + 1, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        let period_end = Utc
            .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
            .single()
            .expect("the first day of the next UTC month is always valid");
        Self {
            limit,
            period_start,
            period_end,
            reset_at: period_end,
        }
    }

    pub fn from_entitlement(
        limit: i64,
        period_start: Option<DateTime<Utc>>,
        period_end: Option<DateTime<Utc>>,
        reset_at: Option<DateTime<Utc>>,
    ) -> Option<Self> {
        let (Some(period_start), Some(period_end)) = (period_start, period_end) else {
            return None;
        };
        if limit < 0 || period_start >= period_end {
            return None;
        }
        Some(Self {
            limit,
            period_start,
            period_end,
            reset_at: reset_at.unwrap_or(period_end),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelQuotaMode {
    /// Self-hosted/disabled deployments do not consume hosted quota.
    Disabled,
    /// A paid entitlement can explicitly opt out of the hosted cap.
    Unlimited,
    /// Free hosted workspaces use a server-side turn cap.
    Limited(i64),
}

impl ChannelQuotaMode {
    pub fn for_messaging_mode(mode: &str) -> Self {
        if mode != "managed" {
            return Self::Disabled;
        }
        match env::var("PATCHBAY_IM_AGENT_TURNS_LIMIT") {
            Ok(value) if value.trim().eq_ignore_ascii_case("unlimited") => Self::Unlimited,
            Ok(value) => value
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|limit| *limit >= 0)
                .map(Self::Limited)
                .unwrap_or(Self::Limited(100)),
            Err(_) => Self::Limited(100),
        }
    }

    pub const fn limit(self) -> Option<i64> {
        match self {
            Self::Limited(limit) => Some(limit),
            Self::Disabled | Self::Unlimited => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("hosted IM turn quota exceeded ({used}/{limit})")]
pub struct ChannelQuotaExceeded {
    pub used: i64,
    pub limit: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelQuotaAdmissionError {
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error("workspace does not exist")]
    WorkspaceMissing,
    #[error(transparent)]
    Exceeded(#[from] ChannelQuotaExceeded),
}

/// Atomically admits one channel Agent turn. The workspace lock is the
/// reservation: it prevents two sessions in the same workspace from both
/// observing the final available slot. The caller must keep the transaction
/// open until its task and message links are written; rollback releases the
/// reservation without a second cleanup race.
pub async fn admit_turn(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    window: ChannelQuotaWindow,
) -> Result<(), ChannelQuotaAdmissionError> {
    // A missing workspace is handled by the normal task transaction as an
    // internal SQL error; it must not be interpreted as free quota.
    let locked = sqlx::query_scalar::<_, Uuid>("SELECT id FROM workspace WHERE id = $1 FOR UPDATE")
        .bind(workspace_id)
        .fetch_optional(&mut *executor)
        .await?;
    if locked.is_none() {
        return Err(ChannelQuotaAdmissionError::WorkspaceMissing);
    }

    // Keep both counts in one statement-level snapshot.  A task can move from
    // an in-flight status to a terminal status without taking the workspace
    // lock; two independent reads could otherwise observe neither category.
    let usage = count_turns_in_window(executor, workspace_id, window).await?;

    if usage.used.saturating_add(usage.reserved) >= window.limit {
        return Err(ChannelQuotaExceeded {
            used: usage.used.saturating_add(usage.reserved),
            limit: window.limit,
        }
        .into());
    }
    Ok(())
}

/// Returns whether this chat session contains an unsealed channel message.
/// Chat task enqueueing is also used by the Web/Desktop direct-chat path, so
/// the hosted IM quota must not charge those first-party messages.
pub async fn has_channel_ingested_message(
    executor: &mut PgConnection,
    chat_session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM chat_message WHERE chat_session_id = $1 AND role = 'user' AND channel_ingested = TRUE AND task_id IS NULL)",
    )
    .bind(chat_session_id)
    .fetch_one(&mut *executor)
    .await
}

/// The two usage buckets returned by one statement-level database snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelQuotaUsage {
    pub used: i64,
    pub reserved: i64,
}

/// Returns accepted hosted channel turns in the supplied entitlement window.
/// A task is usage as soon as its durable row commits, regardless of whether
/// the provider later succeeds, fails, or is cancelled. The caller should use
/// the same workspace lock as [`admit_turn`] when making a blocking decision;
/// read-only usage endpoints may call this without a lock.
pub async fn count_turns_in_window(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    window: ChannelQuotaWindow,
) -> Result<ChannelQuotaUsage, sqlx::Error> {
    let (used, reserved) = sqlx::query_as::<_, (i64, i64)>(
        r#"SELECT
  count(*) FILTER (WHERE task.status NOT IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')),
  count(*) FILTER (WHERE task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred'))
FROM agent_task_queue AS task
JOIN agent ON agent.id = task.agent_id
WHERE task.chat_session_id IS NOT NULL
  AND agent.workspace_id = $1
  AND task.created_at >= $2
  AND task.created_at < $3
  AND EXISTS (
      SELECT 1
      FROM chat_message AS message
      WHERE message.task_id = task.id
        AND message.role = 'user'
        AND message.channel_ingested = TRUE
    )"#,
    )
    .bind(workspace_id)
    .bind(window.period_start)
    .bind(window.period_end)
    .fetch_one(&mut *executor)
    .await?;
    Ok(ChannelQuotaUsage { used, reserved })
}

/// Returns accepted, terminal hosted channel turns in an entitlement window.
pub async fn count_used_turns_in_window(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    window: ChannelQuotaWindow,
) -> Result<i64, sqlx::Error> {
    Ok(count_turns_in_window(executor, workspace_id, window)
        .await?
        .used)
}

/// Returns accepted hosted channel turns whose task is still in flight. These
/// rows are the durable equivalent of Automation's `reserved_count`: they are
/// included in admission while the task is queued, deferred, or executing and
/// disappear from the reservation count when the task reaches a terminal
/// state. At that point the task is included in [`count_used_turns`] instead;
/// a failed/cancelled task does not refund an already accepted turn.
pub async fn count_reserved_turns_in_window(
    executor: &mut PgConnection,
    workspace_id: Uuid,
    window: ChannelQuotaWindow,
) -> Result<i64, sqlx::Error> {
    Ok(count_turns_in_window(executor, workspace_id, window)
        .await?
        .reserved)
}

/// Compatibility helpers for callers that only need the default calendar
/// month. Policy-aware paths must use the `_in_window` functions above.
pub async fn count_used_turns(
    executor: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    count_used_turns_in_window(
        executor,
        workspace_id,
        ChannelQuotaWindow::current_month(0, Utc::now()),
    )
    .await
}

pub async fn count_reserved_turns(
    executor: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    count_reserved_turns_in_window(
        executor,
        workspace_id,
        ChannelQuotaWindow::current_month(0, Utc::now()),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_managed_modes_never_consume_hosted_quota() {
        assert_eq!(
            ChannelQuotaMode::for_messaging_mode("server_configured"),
            ChannelQuotaMode::Disabled
        );
        assert_eq!(
            ChannelQuotaMode::for_messaging_mode("disabled"),
            ChannelQuotaMode::Disabled
        );
    }

    #[test]
    fn limits_are_only_exposed_for_limited_mode() {
        assert_eq!(ChannelQuotaMode::Limited(100).limit(), Some(100));
        assert_eq!(ChannelQuotaMode::Unlimited.limit(), None);
        assert_eq!(ChannelQuotaMode::Disabled.limit(), None);
    }

    #[test]
    fn entitlement_window_requires_valid_cloud_boundaries() {
        let start = Utc::now();
        let end = start + chrono::Duration::days(1);
        let window = ChannelQuotaWindow::from_entitlement(10, Some(start), Some(end), None)
            .expect("valid entitlement window");
        assert_eq!(window.limit, 10);
        assert_eq!(window.reset_at, end);
        assert!(ChannelQuotaWindow::from_entitlement(10, Some(end), Some(start), None).is_none());
        assert!(ChannelQuotaWindow::from_entitlement(10, None, Some(end), None).is_none());
    }
}
