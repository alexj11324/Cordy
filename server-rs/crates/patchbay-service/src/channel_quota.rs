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

use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelQuotaMode {
    /// Self-hosted/disabled deployments do not consume hosted quota.
    Disabled,
    /// A paid entitlement can explicitly opt out of the hosted cap.
    Unlimited,
    /// Free hosted workspaces use a server-side monthly turn cap.
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
#[error("hosted IM monthly turn quota exceeded ({used}/{limit})")]
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
    limit: i64,
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

    let used = count_used_turns(executor, workspace_id).await?;
    let reserved = count_reserved_turns(executor, workspace_id).await?;

    if used.saturating_add(reserved) >= limit {
        return Err(ChannelQuotaExceeded { used, limit }.into());
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

/// Returns accepted, terminal hosted channel turns in the current UTC month.
/// A task is usage as soon as its durable row commits, regardless of whether
/// the provider later succeeds, fails, or is cancelled. The caller should use
/// the same workspace lock as [`admit_turn`] when making a blocking decision;
/// read-only usage endpoints may call this without a lock.
pub async fn count_used_turns(
    executor: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        r#"SELECT count(*)::bigint
FROM agent_task_queue AS task
JOIN agent ON agent.id = task.agent_id
WHERE task.chat_session_id IS NOT NULL
  AND agent.workspace_id = $1
  AND task.status NOT IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  AND task.created_at >= date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
  AND EXISTS (
      SELECT 1
      FROM chat_message AS message
      WHERE message.task_id = task.id
        AND message.role = 'user'
        AND message.channel_ingested = TRUE
  )"#,
    )
    .bind(workspace_id)
    .fetch_one(&mut *executor)
    .await
}

/// Returns accepted hosted channel turns whose task is still in flight. These
/// rows are the durable equivalent of Automation's `reserved_count`: they are
/// included in admission while the task is queued, deferred, or executing and
/// disappear from the reservation count when the task reaches a terminal
/// state. At that point the task is included in [`count_used_turns`] instead;
/// a failed/cancelled task does not refund an already accepted turn.
pub async fn count_reserved_turns(
    executor: &mut PgConnection,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        r#"SELECT count(*)::bigint
FROM agent_task_queue AS task
JOIN agent ON agent.id = task.agent_id
WHERE task.chat_session_id IS NOT NULL
  AND agent.workspace_id = $1
  AND task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  AND task.created_at >= date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
  AND EXISTS (
      SELECT 1
      FROM chat_message AS message
      WHERE message.task_id = task.id
        AND message.role = 'user'
        AND message.channel_ingested = TRUE
  )"#,
    )
    .bind(workspace_id)
    .fetch_one(&mut *executor)
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
}
