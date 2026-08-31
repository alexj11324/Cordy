//! PostgreSQL-backed distributed scheduler. Execution rows are both the
//! ownership primitive and the durable audit log.

mod db;
mod jobs;
mod manager;
mod spec;

#[cfg(test)]
mod contract_tests;

pub use jobs::{
    automation_schedule_dispatch_job, task_usage_hourly_job, AutomationScheduleDispatcher,
    AUTOMATION_SCHEDULE_DISPATCH_JOB, AUTOMATION_TRIGGER_SCOPE,
    DEFAULT_AUTOMATION_SCHEDULE_TIMEZONE, TASK_USAGE_ADVISORY_LOCK_ID, TASK_USAGE_HOURLY_JOB,
};
pub use manager::{
    DbClock, Manager, ManagerOptions, ManagerRuntime, ProcessOutcome, RunReport, SchedulerClock,
    ShutdownOutcome,
};
pub use spec::{
    floor_plan, static_scopes, CatchUpMode, HandlerInput, HandlerResult, JobSpec, LatestPlanInfo,
    Scope, GLOBAL_SCOPE,
};

/// Builds the single production scheduler assembly used by the server.
/// Registration failures are startup errors: silently running only one of
/// the durable jobs would leave production partially scheduled.
pub fn production_manager(
    pool: sqlx::PgPool,
    dispatcher: std::sync::Arc<dyn AutomationScheduleDispatcher>,
) -> anyhow::Result<std::sync::Arc<Manager>> {
    let manager = Manager::new(pool.clone(), ManagerOptions::default());
    manager.register(task_usage_hourly_job(pool.clone()))?;
    manager.register(automation_schedule_dispatch_job(pool, dispatcher))?;
    Ok(manager)
}

#[derive(Debug, thiserror::Error)]
#[error("scheduler lease lost")]
pub struct LeaseLost;
