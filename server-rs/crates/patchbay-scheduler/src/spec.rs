use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

pub type JobFuture<T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'static>>;
pub type ScopeProvider =
    Arc<dyn Fn(CancellationToken, DateTime<Utc>) -> JobFuture<Vec<Scope>> + Send + Sync>;
pub type PlanProvider = Arc<
    dyn Fn(CancellationToken, Scope, DateTime<Utc>, LatestPlanInfo) -> JobFuture<Vec<DateTime<Utc>>>
        + Send
        + Sync,
>;
pub type JobHandler =
    Arc<dyn Fn(CancellationToken, HandlerInput) -> JobFuture<HandlerResult> + Send + Sync>;
pub type Heartbeat = Arc<dyn Fn(CancellationToken) -> JobFuture<()> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUpMode {
    LatestOnly,
    EveryPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Scope {
    pub kind: String,
    pub id: String,
}

impl Scope {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.kind, self.id)
    }
}

pub static GLOBAL_SCOPE: std::sync::LazyLock<Scope> =
    std::sync::LazyLock::new(|| Scope::new("global", "global"));

pub fn static_scopes(scopes: impl IntoIterator<Item = Scope>) -> ScopeProvider {
    let scopes = Arc::new(scopes.into_iter().collect::<Vec<_>>());
    Arc::new(move |_, _| {
        let scopes = scopes.clone();
        Box::pin(async move { Ok(scopes.as_ref().clone()) })
    })
}

#[derive(Clone)]
pub struct HandlerInput {
    pub job_name: String,
    pub scope: Scope,
    pub plan_time: DateTime<Utc>,
    pub attempt: i32,
    pub runner_id: String,
    pub heartbeat: Heartbeat,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HandlerResult {
    pub rows_affected: i64,
    pub result: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct JobSpec {
    pub name: String,
    pub cadence: Duration,
    pub schedule_delay: Duration,
    pub catch_up_mode: CatchUpMode,
    pub catch_up_window: Duration,
    pub max_plans_per_tick: usize,
    pub run_timeout: Duration,
    pub stale_timeout: Duration,
    pub heartbeat_interval: Duration,
    pub allow_stale_reentry: bool,
    pub max_attempts: i32,
    pub retry_backoff: Vec<Duration>,
    pub scopes: ScopeProvider,
    pub plans_for_scope: Option<PlanProvider>,
    pub handler: JobHandler,
}

impl JobSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.name.trim().is_empty(),
            "scheduler: job name is required"
        );
        anyhow::ensure!(
            self.plans_for_scope.is_some() || !self.cadence.is_zero(),
            "scheduler: job {:?}: cadence must be > 0 (or set plans_for_scope)",
            self.name
        );
        anyhow::ensure!(
            !self.run_timeout.is_zero(),
            "scheduler: job {:?}: run_timeout must be > 0",
            self.name
        );
        anyhow::ensure!(
            self.stale_timeout > self.run_timeout,
            "scheduler: job {:?}: stale_timeout must be greater than run_timeout",
            self.name
        );
        anyhow::ensure!(
            !self.heartbeat_interval.is_zero() && self.heartbeat_interval < self.stale_timeout,
            "scheduler: job {:?}: heartbeat_interval must be > 0 and < stale_timeout",
            self.name
        );
        anyhow::ensure!(
            self.max_attempts >= 1,
            "scheduler: job {:?}: max_attempts must be >= 1",
            self.name
        );
        anyhow::ensure!(
            self.plans_for_scope.is_some()
                || self.catch_up_mode != CatchUpMode::EveryPlan
                || self.max_plans_per_tick > 0,
            "scheduler: job {:?}: max_plans_per_tick must be > 0 for every_plan catch-up",
            self.name
        );
        Ok(())
    }

    pub(crate) fn retry_delay(&self, attempt: i32) -> Duration {
        if self.retry_backoff.is_empty() {
            return Duration::ZERO;
        }
        let index = usize::try_from(attempt.saturating_sub(1))
            .unwrap_or_default()
            .min(self.retry_backoff.len() - 1);
        self.retry_backoff[index]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatestPlanInfo {
    pub found: bool,
    pub plan_time: Option<DateTime<Utc>>,
    pub status: String,
    pub attempt: i32,
    pub max_attempts: i32,
    pub next_retry_at: Option<DateTime<Utc>>,
}

impl LatestPlanInfo {
    pub fn retry_eligible(&self, now: DateTime<Utc>) -> bool {
        self.found
            && self.status == "FAILED"
            && self.attempt < self.max_attempts
            && self.next_retry_at.is_none_or(|retry| retry <= now)
    }
}

pub fn floor_plan(eligible: DateTime<Utc>, cadence: Duration) -> DateTime<Utc> {
    if cadence.is_zero() {
        return eligible;
    }
    let cadence_nanos = i128::try_from(cadence.as_nanos()).unwrap_or(i128::MAX);
    let timestamp_nanos = i128::from(
        eligible
            .timestamp_nanos_opt()
            .unwrap_or_else(|| eligible.timestamp().saturating_mul(1_000_000_000)),
    );
    let floored = timestamp_nanos.div_euclid(cadence_nanos) * cadence_nanos;
    let seconds = floored.div_euclid(1_000_000_000);
    let nanos = floored.rem_euclid(1_000_000_000);
    i64::try_from(seconds)
        .ok()
        .zip(u32::try_from(nanos).ok())
        .and_then(|(seconds, nanos)| DateTime::from_timestamp(seconds, nanos))
        .context("floor plan timestamp overflow")
        .unwrap_or(eligible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_plan_is_utc_and_stable() {
        let input = DateTime::parse_from_rfc3339("2026-06-03T08:17:42Z")
            .unwrap_or_else(|error| panic!("timestamp: {error}"))
            .with_timezone(&Utc);
        assert_eq!(
            floor_plan(input, Duration::from_secs(300)),
            DateTime::parse_from_rfc3339("2026-06-03T08:15:00Z")
                .unwrap_or_else(|error| panic!("timestamp: {error}"))
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn retry_eligibility_preserves_failed_plan_cursor() {
        let now = Utc::now();
        let info = LatestPlanInfo {
            found: true,
            status: "FAILED".into(),
            attempt: 1,
            max_attempts: 3,
            next_retry_at: Some(now),
            ..Default::default()
        };
        assert!(info.retry_eligible(now));
    }
}
