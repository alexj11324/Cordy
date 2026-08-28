use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::FutureExt as _;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::{self, Claim, ClaimKind};
use crate::spec::{floor_plan, HandlerInput, HandlerResult, JobSpec, LatestPlanInfo, Scope};
use crate::LeaseLost;

const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait]
pub trait SchedulerClock: Send + Sync {
    async fn now(&self) -> anyhow::Result<DateTime<Utc>>;
}

pub struct DbClock {
    pool: sqlx::PgPool,
}

impl DbClock {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SchedulerClock for DbClock {
    async fn now(&self) -> anyhow::Result<DateTime<Utc>> {
        db::db_now(&self.pool).await
    }
}

pub struct ManagerOptions {
    pub runner_id: String,
    pub tick_interval: Duration,
    pub shutdown_timeout: Duration,
    pub clock: Option<Arc<dyn SchedulerClock>>,
}

impl Default for ManagerOptions {
    fn default() -> Self {
        Self {
            runner_id: String::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            clock: None,
        }
    }
}

pub struct Manager {
    pool: sqlx::PgPool,
    runner_id: String,
    tick_interval: Duration,
    shutdown_timeout: Duration,
    clock: Arc<dyn SchedulerClock>,
    jobs: RwLock<HashMap<String, Arc<JobSpec>>>,
}

impl Manager {
    pub fn new(pool: sqlx::PgPool, mut options: ManagerOptions) -> Arc<Self> {
        if options.runner_id.trim().is_empty() {
            options.runner_id = patchbay_db::dbid::new_v7().to_string();
        }
        if options.tick_interval.is_zero() {
            options.tick_interval = DEFAULT_TICK_INTERVAL;
        }
        if options.shutdown_timeout.is_zero() {
            options.shutdown_timeout = DEFAULT_SHUTDOWN_TIMEOUT;
        }
        let clock = options
            .clock
            .unwrap_or_else(|| Arc::new(DbClock::new(pool.clone())));
        Arc::new(Self {
            pool,
            runner_id: options.runner_id,
            tick_interval: options.tick_interval,
            shutdown_timeout: options.shutdown_timeout,
            clock,
            jobs: RwLock::new(HashMap::new()),
        })
    }

    pub fn register(&self, job: JobSpec) -> anyhow::Result<()> {
        job.validate()?;
        let name = job.name.clone();
        let mut jobs = self
            .jobs
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        anyhow::ensure!(
            !jobs.contains_key(&name),
            "scheduler: duplicate job name {name:?}"
        );
        jobs.insert(name, Arc::new(job));
        Ok(())
    }

    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> anyhow::Result<ManagerRuntime> {
        anyhow::ensure!(
            !self.snapshot().is_empty(),
            "scheduler: refusing to start without registered jobs"
        );
        let shutdown_timeout = self.shutdown_timeout;
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        Ok(ManagerRuntime {
            cancel,
            task: Some(task),
            shutdown_timeout,
        })
    }

    async fn run(self: Arc<Self>, cancel: CancellationToken) {
        tracing::info!(
            runner_id = %self.runner_id,
            tick_interval = ?self.tick_interval,
            jobs = self.snapshot().len(),
            "scheduler starting"
        );
        let mut ticker = tokio::time::interval(self.tick_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            match self.run_once(&cancel).await {
                Ok(_) => {}
                Err(_) if cancel.is_cancelled() => return,
                Err(error) => tracing::warn!(%error, "scheduler tick error"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => {}
            }
        }
    }

    /// Executes one complete scheduler tick using the injected canonical
    /// clock. Job-local failures are isolated and accumulated in the report.
    pub async fn run_once(&self, cancel: &CancellationToken) -> anyhow::Result<RunReport> {
        if cancel.is_cancelled() {
            anyhow::bail!("scheduler cancelled");
        }
        let now = self.clock.now().await?;
        let mut report = RunReport::default();
        for job in self.snapshot() {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            if let Err(error) = self.run_job(cancel, &job, now, &mut report).await {
                report.job_errors += 1;
                tracing::warn!(job = job.name, %error, "scheduler job tick error");
            }
        }
        Ok(report)
    }

    async fn run_job(
        &self,
        cancel: &CancellationToken,
        job: &JobSpec,
        now: DateTime<Utc>,
        report: &mut RunReport,
    ) -> anyhow::Result<()> {
        let scopes = (job.scopes)(cancel.child_token(), now)
            .await
            .with_context(|| format!("scheduler: scope provider for {:?}", job.name))?;
        match db::mark_stale_failed(&self.pool, &job.name, now).await {
            Ok(affected) => report.stale_closed += affected,
            Err(error) => {
                report.job_errors += 1;
                tracing::warn!(job = job.name, %error, "scheduler: mark stale failed");
            }
        }
        for scope in scopes {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            let plans = match self.plans_for_tick(cancel, job, &scope, now).await {
                Ok(plans) => plans,
                Err(error) => {
                    report.job_errors += 1;
                    tracing::warn!(job = job.name, %scope, %error, "scheduler plan computation");
                    continue;
                }
            };
            report.plans_considered += plans.len();
            for plan_time in plans {
                let outcome = self
                    .process_next(cancel, job, scope.clone(), plan_time, now)
                    .await;
                report.observe(outcome);
            }
        }
        Ok(())
    }

    pub async fn plans_for_tick(
        &self,
        cancel: &CancellationToken,
        job: &JobSpec,
        scope: &Scope,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Vec<DateTime<Utc>>> {
        if let Some(provider) = &job.plans_for_scope {
            let latest = db::latest_plan(&self.pool, &job.name, scope).await?;
            let mut plans = provider(cancel.child_token(), scope.clone(), now, latest)
                .await
                .with_context(|| format!("scheduler: plans hook for {:?}", job.name))?;
            if job.max_plans_per_tick > 0 && plans.len() > job.max_plans_per_tick {
                plans.truncate(job.max_plans_per_tick);
            }
            return Ok(plans);
        }

        let eligible = subtract_duration(now, job.schedule_delay)?;
        let latest_due = floor_plan(eligible, job.cadence);
        if latest_due > eligible {
            return Ok(Vec::new());
        }
        match job.catch_up_mode {
            crate::CatchUpMode::LatestOnly => Ok(vec![latest_due]),
            crate::CatchUpMode::EveryPlan => {
                let info = db::latest_plan(&self.pool, &job.name, scope).await?;
                every_plan_schedule(job, info, now, latest_due)
            }
        }
    }

    /// Claims and processes exactly one plan. The lease-token terminal guard
    /// makes this safe to call concurrently from multiple managers.
    pub async fn process_next(
        &self,
        cancel: &CancellationToken,
        job: &JobSpec,
        scope: Scope,
        plan_time: DateTime<Utc>,
        db_time: DateTime<Utc>,
    ) -> ProcessOutcome {
        if cancel.is_cancelled() {
            return ProcessOutcome::Cancelled;
        }
        let claim =
            match db::try_claim(&self.pool, job, &scope, plan_time, db_time, &self.runner_id).await
            {
                Ok(claim) => claim,
                Err(error) => return ProcessOutcome::ClaimError(error.to_string()),
            };
        if claim.kind == ClaimKind::Conflicted {
            return ProcessOutcome::Conflicted;
        }
        self.run_claimed(cancel, job, scope, plan_time, claim).await
    }

    async fn run_claimed(
        &self,
        root_cancel: &CancellationToken,
        job: &JobSpec,
        scope: Scope,
        plan_time: DateTime<Utc>,
        claim: Claim,
    ) -> ProcessOutcome {
        let run_cancel = root_cancel.child_token();
        let heartbeat_pool = self.pool.clone();
        let heartbeat_cancel = run_cancel.clone();
        let stale_timeout = job.stale_timeout;
        let heartbeat = Arc::new(move |cancel: CancellationToken| {
            let pool = heartbeat_pool.clone();
            let heartbeat_cancel = heartbeat_cancel.clone();
            Box::pin(async move {
                tokio::select! {
                    _ = heartbeat_cancel.cancelled() => Err(anyhow::anyhow!("scheduler heartbeat cancelled")),
                    _ = cancel.cancelled() => Err(anyhow::anyhow!("scheduler heartbeat cancelled")),
                    result = db::heartbeat(&pool, claim.id, claim.lease_token, stale_timeout) => result,
                }
            }) as crate::spec::JobFuture<()>
        });
        let input = HandlerInput {
            job_name: job.name.clone(),
            scope,
            plan_time,
            attempt: claim.attempt,
            runner_id: self.runner_id.clone(),
            heartbeat,
        };
        let handler = job.handler.clone();
        let handler_cancel = run_cancel.clone();
        let handler_future = async move {
            match catch_unwind(AssertUnwindSafe(|| handler(handler_cancel, input))) {
                Ok(future) => AssertUnwindSafe(future).catch_unwind().await,
                Err(panic) => Err(panic),
            }
        };
        tokio::pin!(handler_future);
        let timeout = tokio::time::sleep(job.run_timeout);
        tokio::pin!(timeout);
        let mut heartbeat_tick = tokio::time::interval(job.heartbeat_interval);
        heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat_tick.tick().await;
        let started = Instant::now();

        let completion = loop {
            tokio::select! {
                _ = root_cancel.cancelled() => {
                    run_cancel.cancel();
                    break HandlerCompletion::Cancelled;
                }
                _ = &mut timeout => {
                    run_cancel.cancel();
                    break HandlerCompletion::TimedOut;
                }
                result = &mut handler_future => {
                    break match result {
                        Ok(Ok(value)) => HandlerCompletion::Success(value),
                        Ok(Err(error)) => HandlerCompletion::Failed(error),
                        Err(panic) => HandlerCompletion::Panicked(panic_message(panic)),
                    };
                }
                _ = heartbeat_tick.tick() => {
                    match tokio::time::timeout(
                        HEARTBEAT_QUERY_TIMEOUT,
                        db::heartbeat(&self.pool, claim.id, claim.lease_token, job.stale_timeout),
                    ).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) if error.downcast_ref::<LeaseLost>().is_some() => {
                            run_cancel.cancel();
                            break HandlerCompletion::LeaseLost;
                        }
                        Ok(Err(error)) => tracing::warn!(job = job.name, %error, "scheduler heartbeat error"),
                        Err(_) => tracing::warn!(job = job.name, "scheduler heartbeat timed out"),
                    }
                }
            }
        };
        run_cancel.cancel();
        let duration_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
        if matches!(completion, HandlerCompletion::LeaseLost) {
            return ProcessOutcome::LeaseLost;
        }
        let terminal_time = match self.clock.now().await {
            Ok(now) => now,
            Err(error) => {
                tracing::warn!(%error, "scheduler terminal DB clock unavailable; using local UTC");
                Utc::now()
            }
        };

        match completion {
            HandlerCompletion::Success(result) => {
                match db::finish_success(&self.pool, claim, terminal_time, duration_ms, result)
                    .await
                {
                    Ok(()) => ProcessOutcome::Succeeded,
                    Err(error) if error.downcast_ref::<LeaseLost>().is_some() => {
                        ProcessOutcome::LeaseLost
                    }
                    Err(error) => ProcessOutcome::TerminalError(error.to_string()),
                }
            }
            failure => {
                let (code, message) = failure.code_and_message();
                let next_retry_at = if claim.attempt < job.max_attempts {
                    add_duration(terminal_time, job.retry_delay(claim.attempt)).ok()
                } else {
                    None
                };
                match db::finish_failure(
                    &self.pool,
                    claim,
                    terminal_time,
                    duration_ms,
                    code,
                    &message,
                    next_retry_at,
                )
                .await
                {
                    Ok(()) => ProcessOutcome::Failed {
                        error_code: code.to_string(),
                        will_retry: claim.attempt < job.max_attempts,
                    },
                    Err(error) if error.downcast_ref::<LeaseLost>().is_some() => {
                        ProcessOutcome::LeaseLost
                    }
                    Err(error) => ProcessOutcome::TerminalError(error.to_string()),
                }
            }
        }
    }

    fn snapshot(&self) -> Vec<Arc<JobSpec>> {
        let jobs = self
            .jobs
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut snapshot = jobs.values().cloned().collect::<Vec<_>>();
        snapshot.sort_by(|left, right| left.name.cmp(&right.name));
        snapshot
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReport {
    pub plans_considered: usize,
    pub claimed: usize,
    pub conflicted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub lease_lost: usize,
    pub claim_errors: usize,
    pub terminal_errors: usize,
    pub stale_closed: u64,
    pub job_errors: usize,
    pub cancelled: bool,
}

impl RunReport {
    fn observe(&mut self, outcome: ProcessOutcome) {
        match outcome {
            ProcessOutcome::Succeeded => {
                self.claimed += 1;
                self.succeeded += 1;
            }
            ProcessOutcome::Failed { .. } => {
                self.claimed += 1;
                self.failed += 1;
            }
            ProcessOutcome::LeaseLost => {
                self.claimed += 1;
                self.lease_lost += 1;
            }
            ProcessOutcome::Conflicted => self.conflicted += 1,
            ProcessOutcome::ClaimError(_) => self.claim_errors += 1,
            ProcessOutcome::TerminalError(_) => {
                self.claimed += 1;
                self.terminal_errors += 1;
            }
            ProcessOutcome::Cancelled => self.cancelled = true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    Succeeded,
    Failed {
        error_code: String,
        will_retry: bool,
    },
    LeaseLost,
    Conflicted,
    ClaimError(String),
    TerminalError(String),
    Cancelled,
}

enum HandlerCompletion {
    Success(HandlerResult),
    Failed(anyhow::Error),
    TimedOut,
    Cancelled,
    LeaseLost,
    Panicked(String),
}

impl HandlerCompletion {
    fn code_and_message(&self) -> (&'static str, String) {
        match self {
            Self::Failed(error) if error.downcast_ref::<LeaseLost>().is_some() => {
                ("lease_lost", error.to_string())
            }
            Self::Failed(error) => ("handler_error", error.to_string()),
            Self::TimedOut => ("run_timeout", "scheduler run timeout".into()),
            Self::Cancelled => ("canceled", "scheduler run cancelled".into()),
            Self::Panicked(message) => ("handler_panic", message.clone()),
            Self::LeaseLost => ("lease_lost", "scheduler lease lost".into()),
            Self::Success(_) => ("handler_error", "invalid success classification".into()),
        }
    }
}

pub struct ManagerRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    shutdown_timeout: Duration,
}

impl ManagerRuntime {
    pub async fn shutdown(mut self) -> ShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return ShutdownOutcome::Panicked;
        };
        match tokio::time::timeout(self.shutdown_timeout, &mut task).await {
            Ok(Ok(())) => ShutdownOutcome::Stopped,
            Ok(Err(_)) => ShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                ShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for ManagerRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

fn every_plan_schedule(
    job: &JobSpec,
    latest: LatestPlanInfo,
    now: DateTime<Utc>,
    latest_due: DateTime<Utc>,
) -> anyhow::Result<Vec<DateTime<Utc>>> {
    let oldest_allowed = if job.catch_up_window.is_zero() {
        latest_due
    } else {
        subtract_duration(now, job.catch_up_window)?
    };
    let mut start = if latest.retry_eligible(now) {
        latest.plan_time.unwrap_or(latest_due)
    } else if let Some(plan_time) = latest.plan_time {
        add_duration(plan_time, job.cadence)?
    } else {
        latest_due
    };
    if start < oldest_allowed {
        start = floor_plan(oldest_allowed, job.cadence);
        if start < oldest_allowed {
            start = add_duration(start, job.cadence)?;
        }
    }
    let mut plans = Vec::new();
    while start <= latest_due && plans.len() < job.max_plans_per_tick {
        plans.push(start);
        start = add_duration(start, job.cadence)?;
    }
    Ok(plans)
}

fn add_duration(value: DateTime<Utc>, duration: Duration) -> anyhow::Result<DateTime<Utc>> {
    value
        .checked_add_signed(chrono::Duration::from_std(duration)?)
        .context("scheduler timestamp overflow")
}

fn subtract_duration(value: DateTime<Utc>, duration: Duration) -> anyhow::Result<DateTime<Utc>> {
    value
        .checked_sub_signed(chrono::Duration::from_std(duration)?)
        .context("scheduler timestamp overflow")
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("scheduler handler panic: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("scheduler handler panic: {message}")
    } else {
        "scheduler handler panic: non-string payload".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{static_scopes, CatchUpMode, GLOBAL_SCOPE};

    fn test_job() -> JobSpec {
        JobSpec {
            name: "test_job".into(),
            cadence: Duration::from_secs(300),
            schedule_delay: Duration::ZERO,
            catch_up_mode: CatchUpMode::EveryPlan,
            catch_up_window: Duration::from_secs(3600),
            max_plans_per_tick: 4,
            run_timeout: Duration::from_secs(60),
            stale_timeout: Duration::from_secs(120),
            heartbeat_interval: Duration::from_secs(30),
            allow_stale_reentry: true,
            max_attempts: 3,
            retry_backoff: vec![Duration::from_secs(1)],
            scopes: static_scopes([GLOBAL_SCOPE.clone()]),
            plans_for_scope: None,
            handler: Arc::new(|_, _| Box::pin(async { Ok(HandlerResult::default()) })),
        }
    }

    #[test]
    fn every_plan_retry_keeps_same_cursor_without_sleep() {
        let now = DateTime::parse_from_rfc3339("2026-06-03T08:17:42Z")
            .unwrap_or_else(|error| panic!("timestamp: {error}"))
            .with_timezone(&Utc);
        let latest_due = floor_plan(now, Duration::from_secs(300));
        let latest = LatestPlanInfo {
            found: true,
            plan_time: Some(latest_due),
            status: "FAILED".into(),
            attempt: 1,
            max_attempts: 3,
            next_retry_at: Some(now),
        };
        let plans = every_plan_schedule(&test_job(), latest, now, latest_due)
            .unwrap_or_else(|error| panic!("plans: {error}"));
        assert_eq!(plans, vec![latest_due]);
    }

    #[test]
    fn panic_payload_classifies_for_audit() {
        assert_eq!(
            panic_message(Box::new("boom")),
            "scheduler handler panic: boom"
        );
    }
}
