//! PostgreSQL-backed distributed scheduler. Execution rows are both the
//! ownership primitive and the durable audit log.

mod db;
mod jobs;
mod manager;
mod spec;

#[cfg(test)]
mod contract_tests;

pub use jobs::{
    autopilot_schedule_dispatch_job, task_usage_hourly_job, AutopilotScheduleDispatcher,
    AUTOPILOT_SCHEDULE_DISPATCH_JOB, AUTOPILOT_TRIGGER_SCOPE, DEFAULT_AUTOPILOT_SCHEDULE_TIMEZONE,
    TASK_USAGE_ADVISORY_LOCK_ID, TASK_USAGE_HOURLY_JOB,
};
pub use manager::{
    DbClock, Manager, ManagerOptions, ManagerRuntime, ProcessOutcome, RunReport, SchedulerClock,
    ShutdownOutcome,
};
pub use spec::{
    floor_plan, static_scopes, CatchUpMode, HandlerInput, HandlerResult, JobSpec, LatestPlanInfo,
    Scope, GLOBAL_SCOPE,
};

#[derive(Debug, thiserror::Error)]
#[error("scheduler lease lost")]
pub struct LeaseLost;
