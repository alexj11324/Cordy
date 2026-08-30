#![allow(clippy::expect_used)]
// Database contract fixtures need contextual fail-fast messages when setup or query invariants break.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use patchbay_db::dbid::new_v7;
use patchbay_db::models::{Automation, AutomationRun};
use serde_json::Value;
use sqlx::{PgPool, Row as _};
use tokio::sync::{Barrier, Notify};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::spec::JobHandler;
use crate::{
    static_scopes, CatchUpMode, HandlerResult, JobSpec, Manager, ManagerOptions, ProcessOutcome,
    Scope, ShutdownOutcome,
};

struct ExecutionRows {
    pool: PgPool,
    prefix: String,
}

impl ExecutionRows {
    async fn required() -> Self {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for scheduler production contracts");
        let pool = PgPool::connect(&url)
            .await
            .expect("scheduler production contract requires a reachable migrated PostgreSQL");
        Self {
            pool,
            prefix: format!("rust_contract_{}", new_v7()),
        }
    }

    fn job_name(&self, suffix: &str) -> String {
        format!("{}_{}", self.prefix, suffix)
    }

    async fn cleanup(&self) {
        sqlx::query("DELETE FROM sys_cron_executions WHERE left(job_name, length($1)) = $1")
            .bind(&self.prefix)
            .execute(&self.pool)
            .await
            .expect("clean scheduler contract rows");
    }
}

impl Drop for ExecutionRows {
    fn drop(&mut self) {
        let pool = self.pool.clone();
        let prefix = self.prefix.clone();
        // Detached cleanup races test-runtime teardown and can leak audit rows
        // into the next scheduler contract. Complete it before Drop returns.
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build scheduler cleanup executor");
            runtime.block_on(async move {
                let _ = sqlx::query(
                    "DELETE FROM sys_cron_executions WHERE left(job_name, length($1)) = $1",
                )
                .bind(prefix)
                .execute(&pool)
                .await;
            });
        })
        .join();
    }
}

fn manager(pool: &PgPool, runner_id: &str) -> Arc<Manager> {
    Manager::new(
        pool.clone(),
        ManagerOptions {
            runner_id: runner_id.into(),
            tick_interval: Duration::from_secs(3_600),
            shutdown_timeout: Duration::from_secs(2),
            clock: None,
        },
    )
}

fn fixed_plan_job(name: String, plan_time: DateTime<Utc>, handler: JobHandler) -> JobSpec {
    JobSpec {
        name,
        cadence: Duration::from_secs(60),
        schedule_delay: Duration::ZERO,
        catch_up_mode: CatchUpMode::EveryPlan,
        catch_up_window: Duration::from_secs(3_600),
        max_plans_per_tick: 1,
        run_timeout: Duration::from_secs(30),
        stale_timeout: Duration::from_secs(60),
        heartbeat_interval: Duration::from_secs(10),
        allow_stale_reentry: true,
        max_attempts: 3,
        retry_backoff: vec![Duration::ZERO],
        scopes: static_scopes([Scope::new("workspace", "scheduler-contract")]),
        plans_for_scope: Some(Arc::new(move |_, _, _, _| {
            Box::pin(async move { Ok(vec![plan_time]) })
        })),
        handler,
    }
}

async fn db_now(pool: &PgPool) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT now()")
        .fetch_one(pool)
        .await
        .expect("read PostgreSQL clock")
}

struct ContractDispatcher;

#[async_trait::async_trait]
impl crate::AutomationScheduleDispatcher for ContractDispatcher {
    async fn dispatch_automation_for_plan(
        &self,
        _automation: &Automation,
        _trigger_id: Uuid,
        _source: &str,
        _payload: &Value,
        _planned_at: DateTime<Utc>,
    ) -> anyhow::Result<AutomationRun> {
        anyhow::bail!("contract dispatcher must not be called")
    }
}

#[tokio::test]
async fn production_scheduler_assembly_registers_both_real_jobs() {
    let rows = ExecutionRows::required().await;
    let dispatcher: Arc<dyn crate::AutomationScheduleDispatcher> = Arc::new(ContractDispatcher);
    let scheduler = crate::production_manager(rows.pool.clone(), dispatcher.clone())
        .expect("build production scheduler assembly");

    let task_usage_error = scheduler
        .register(crate::task_usage_hourly_job(rows.pool.clone()))
        .expect_err("production assembly omitted task usage job");
    assert!(task_usage_error
        .to_string()
        .contains(crate::TASK_USAGE_HOURLY_JOB));
    let automation_error = scheduler
        .register(crate::automation_schedule_dispatch_job(
            rows.pool.clone(),
            dispatcher,
        ))
        .expect_err("production assembly omitted automation schedule job");
    assert!(automation_error
        .to_string()
        .contains(crate::AUTOMATION_SCHEDULE_DISPATCH_JOB));

    let cancel = CancellationToken::new();
    cancel.cancel();
    let runtime = scheduler
        .start(cancel)
        .expect("start production scheduler assembly");
    assert_eq!(runtime.shutdown().await, ShutdownOutcome::Stopped);
    rows.cleanup().await;
}

#[tokio::test]
async fn production_scheduler_single_winner_audit_and_runtime_shutdown() {
    let rows = ExecutionRows::required().await;
    let plan_time = db_now(&rows.pool).await;
    let job_name = rows.job_name("single_winner");
    let executions = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let executions = executions.clone();
        Arc::new(move |_, _| {
            let executions = executions.clone();
            Box::pin(async move {
                executions.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                let mut result = serde_json::Map::new();
                result.insert("worker".into(), serde_json::json!("rust"));
                Ok(HandlerResult {
                    rows_affected: 7,
                    result,
                })
            })
        })
    };
    let job = Arc::new(fixed_plan_job(job_name.clone(), plan_time, handler));
    let first = manager(&rows.pool, "runner-a");
    let second = manager(&rows.pool, "runner-b");
    let now = db_now(&rows.pool).await;
    let cancel = CancellationToken::new();
    let first_run = tokio::spawn({
        let first = first.clone();
        let job = job.clone();
        let cancel = cancel.clone();
        async move {
            first
                .process_next(
                    &cancel,
                    &job,
                    Scope::new("workspace", "scheduler-contract"),
                    plan_time,
                    now,
                )
                .await
        }
    });
    let second_run = tokio::spawn({
        let second = second.clone();
        let job = job.clone();
        let cancel = cancel.clone();
        async move {
            second
                .process_next(
                    &cancel,
                    &job,
                    Scope::new("workspace", "scheduler-contract"),
                    plan_time,
                    now,
                )
                .await
        }
    });
    let outcomes = [
        first_run.await.expect("first manager join"),
        second_run.await.expect("second manager join"),
    ];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ProcessOutcome::Succeeded))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ProcessOutcome::Conflicted))
            .count(),
        1
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let audit = sqlx::query(
        "SELECT status, attempt, runner_id, rows_affected, result, error_code, finished_at \
         FROM sys_cron_executions WHERE job_name = $1",
    )
    .bind(&job_name)
    .fetch_one(&rows.pool)
    .await
    .expect("single scheduler audit row");
    assert_eq!(audit.get::<String, _>("status"), "SUCCESS");
    assert_eq!(audit.get::<i32, _>("attempt"), 1);
    assert!(matches!(
        audit.get::<String, _>("runner_id").as_str(),
        "runner-a" | "runner-b"
    ));
    assert_eq!(audit.get::<i64, _>("rows_affected"), 7);
    assert_eq!(
        audit.get::<serde_json::Value, _>("result"),
        serde_json::json!({"worker": "rust"})
    );
    assert!(audit.get::<Option<String>, _>("error_code").is_none());
    assert!(audit
        .get::<Option<DateTime<Utc>>, _>("finished_at")
        .is_some());

    let runtime_name = rows.job_name("runtime");
    let runtime_plan = db_now(&rows.pool).await;
    let runtime_entered = Arc::new(Notify::new());
    let runtime_job = fixed_plan_job(runtime_name.clone(), runtime_plan, {
        let runtime_entered = runtime_entered.clone();
        Arc::new(move |_, _| {
            runtime_entered.notify_one();
            Box::pin(async { std::future::pending::<anyhow::Result<HandlerResult>>().await })
        })
    });
    let runtime_manager = manager(&rows.pool, "runtime-runner");
    runtime_manager
        .register(runtime_job)
        .expect("register production scheduler job");
    let runtime = runtime_manager
        .start(CancellationToken::new())
        .expect("start production scheduler runtime");
    tokio::time::timeout(Duration::from_secs(2), runtime_entered.notified())
        .await
        .expect("runtime handler entered");
    assert_eq!(runtime.shutdown().await, ShutdownOutcome::Stopped);
    let runtime_status: String =
        sqlx::query_scalar("SELECT status FROM sys_cron_executions WHERE job_name = $1")
            .bind(&runtime_name)
            .fetch_one(&rows.pool)
            .await
            .expect("runtime audit row");
    assert_eq!(runtime_status, "FAILED");
    rows.cleanup().await;
}

#[tokio::test]
async fn production_scheduler_retries_same_plan_and_classifies_failures() {
    let rows = ExecutionRows::required().await;
    let retry_name = rows.job_name("retry");
    let retry_plan = db_now(&rows.pool).await;
    let mut retry_job = fixed_plan_job(
        retry_name.clone(),
        retry_plan,
        Arc::new(|_, input| {
            Box::pin(async move {
                if input.attempt == 1 {
                    anyhow::bail!("first attempt fails")
                }
                Ok(HandlerResult::default())
            })
        }),
    );
    // Exercise the production plan cursor: the first tick creates the plan,
    // while the second tick must rediscover that exact plan from
    // LatestPlanInfo as a retry rather than receiving a synthetic constant.
    retry_job.plans_for_scope = Some(Arc::new(move |_, _, now, latest| {
        Box::pin(async move {
            if !latest.found {
                Ok(vec![retry_plan])
            } else if latest.retry_eligible(now) {
                Ok(vec![latest
                    .plan_time
                    .expect("retry cursor includes plan time")])
            } else {
                Ok(Vec::new())
            }
        })
    }));
    let retry_manager = manager(&rows.pool, "retry-runner");
    retry_manager
        .register(retry_job)
        .expect("register retry job");
    let first = retry_manager
        .run_once(&CancellationToken::new())
        .await
        .expect("first retry tick");
    assert_eq!((first.failed, first.succeeded), (1, 0));
    let second = retry_manager
        .run_once(&CancellationToken::new())
        .await
        .expect("second retry tick");
    assert_eq!((second.failed, second.succeeded), (0, 1));
    let retry_audit = sqlx::query(
        "SELECT status, attempt, next_retry_at, error_code FROM sys_cron_executions \
         WHERE job_name = $1 AND plan_time = $2",
    )
    .bind(&retry_name)
    .bind(retry_plan)
    .fetch_one(&rows.pool)
    .await
    .expect("retry audit row");
    assert_eq!(retry_audit.get::<String, _>("status"), "SUCCESS");
    assert_eq!(retry_audit.get::<i32, _>("attempt"), 2);
    assert!(retry_audit
        .get::<Option<DateTime<Utc>>, _>("next_retry_at")
        .is_none());
    assert!(retry_audit.get::<Option<String>, _>("error_code").is_none());

    let panic_name = rows.job_name("panic");
    let panic_plan = db_now(&rows.pool).await;
    let panic_job = fixed_plan_job(
        panic_name.clone(),
        panic_plan,
        Arc::new(|_, _| Box::pin(async { panic!("scheduler contract panic") })),
    );
    let panic_outcome = manager(&rows.pool, "panic-runner")
        .process_next(
            &CancellationToken::new(),
            &panic_job,
            Scope::new("workspace", "scheduler-contract"),
            panic_plan,
            db_now(&rows.pool).await,
        )
        .await;
    assert!(matches!(
        panic_outcome,
        ProcessOutcome::Failed {
            ref error_code,
            will_retry: true
        } if error_code == "handler_panic"
    ));

    let timeout_name = rows.job_name("timeout");
    let timeout_plan = db_now(&rows.pool).await;
    let mut timeout_job = fixed_plan_job(
        timeout_name.clone(),
        timeout_plan,
        Arc::new(|_, _| Box::pin(std::future::pending())),
    );
    timeout_job.run_timeout = Duration::from_millis(20);
    timeout_job.stale_timeout = Duration::from_secs(1);
    timeout_job.heartbeat_interval = Duration::from_millis(100);
    let timeout_outcome = manager(&rows.pool, "timeout-runner")
        .process_next(
            &CancellationToken::new(),
            &timeout_job,
            Scope::new("workspace", "scheduler-contract"),
            timeout_plan,
            db_now(&rows.pool).await,
        )
        .await;
    assert!(matches!(
        timeout_outcome,
        ProcessOutcome::Failed {
            ref error_code,
            will_retry: true
        } if error_code == "run_timeout"
    ));

    let cancel_name = rows.job_name("cancel");
    let cancel_plan = db_now(&rows.pool).await;
    let entered = Arc::new(Notify::new());
    let cancel_job = Arc::new(fixed_plan_job(cancel_name.clone(), cancel_plan, {
        let entered = entered.clone();
        Arc::new(move |_, _| {
            let entered = entered.clone();
            Box::pin(async move {
                entered.notify_one();
                std::future::pending().await
            })
        })
    }));
    let root_cancel = CancellationToken::new();
    let cancel_run = tokio::spawn({
        let pool = rows.pool.clone();
        let cancel_job = cancel_job.clone();
        let root_cancel = root_cancel.clone();
        async move {
            manager(&pool, "cancel-runner")
                .process_next(
                    &root_cancel,
                    &cancel_job,
                    Scope::new("workspace", "scheduler-contract"),
                    cancel_plan,
                    db_now(&pool).await,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("cancel handler entered");
    root_cancel.cancel();
    let cancel_outcome = cancel_run.await.expect("cancel run join");
    assert!(matches!(
        cancel_outcome,
        ProcessOutcome::Failed {
            ref error_code,
            will_retry: true
        } if error_code == "canceled"
    ));

    let failures = sqlx::query(
        "SELECT job_name, status, error_code FROM sys_cron_executions \
         WHERE job_name = ANY($1) ORDER BY job_name",
    )
    .bind(vec![panic_name, timeout_name, cancel_name])
    .fetch_all(&rows.pool)
    .await
    .expect("classified failure audit rows");
    let codes = failures
        .into_iter()
        .map(|row| {
            assert_eq!(row.get::<String, _>("status"), "FAILED");
            (
                row.get::<String, _>("job_name"),
                row.get::<String, _>("error_code"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(codes.len(), 3);
    assert!(codes.iter().any(|(_, code)| code == "handler_panic"));
    assert!(codes.iter().any(|(_, code)| code == "run_timeout"));
    assert!(codes.iter().any(|(_, code)| code == "canceled"));
    rows.cleanup().await;
}

#[tokio::test]
async fn production_scheduler_linearizes_live_heartbeat_against_stale_reclaim() {
    let rows = ExecutionRows::required().await;
    let job_name = rows.job_name("stale");
    let plan_time = db_now(&rows.pool).await;
    let first_entered = Arc::new(Notify::new());
    let race_start = Arc::new(Barrier::new(3));
    let job = Arc::new(fixed_plan_job(job_name.clone(), plan_time, {
        let first_entered = first_entered.clone();
        let race_start = race_start.clone();
        Arc::new(move |_, input| {
            let first_entered = first_entered.clone();
            let race_start = race_start.clone();
            Box::pin(async move {
                if input.attempt == 1 {
                    first_entered.notify_one();
                    race_start.wait().await;
                    // Race the live owner's real heartbeat against the stale
                    // close/reclaim tick. Exactly one linearized owner may
                    // retain or reclaim this execution row.
                    (input.heartbeat)(CancellationToken::new()).await?;
                }
                Ok(HandlerResult::default())
            })
        })
    }));
    let first_cancel = CancellationToken::new();
    let first_run = tokio::spawn({
        let pool = rows.pool.clone();
        let job = job.clone();
        let first_cancel = first_cancel.clone();
        async move {
            manager(&pool, "stale-runner-a")
                .process_next(
                    &first_cancel,
                    &job,
                    Scope::new("workspace", "scheduler-contract"),
                    plan_time,
                    db_now(&pool).await,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), first_entered.notified())
        .await
        .expect("first lease owner entered");
    sqlx::query(
        "UPDATE sys_cron_executions SET stale_after = now() - interval '1 second' \
         WHERE job_name = $1 AND status = 'RUNNING'",
    )
    .bind(&job_name)
    .execute(&rows.pool)
    .await
    .expect("expire first lease");

    let second = manager(&rows.pool, "stale-runner-b");
    second.register((*job).clone()).expect("register stale job");
    let second_run = tokio::spawn({
        let race_start = race_start.clone();
        async move {
            race_start.wait().await;
            second
                .run_once(&CancellationToken::new())
                .await
                .expect("stale reclaim tick")
        }
    });
    race_start.wait().await;
    let first_outcome = first_run.await.expect("live owner join");
    let report = second_run.await.expect("stale manager join");
    match &first_outcome {
        ProcessOutcome::Succeeded => {
            assert_eq!(report.stale_closed, 0);
            assert_eq!(report.conflicted, 1);
            assert_eq!(report.succeeded, 0);
        }
        ProcessOutcome::LeaseLost => {
            assert_eq!(report.stale_closed, 1);
            assert_eq!(report.succeeded, 1);
        }
        other => panic!("unexpected live-owner race outcome: {other:?}"),
    }

    let audit = sqlx::query(
        "SELECT status, attempt, runner_id, error_code FROM sys_cron_executions \
         WHERE job_name = $1 AND plan_time = $2",
    )
    .bind(&job_name)
    .bind(plan_time)
    .fetch_one(&rows.pool)
    .await
    .expect("reclaimed scheduler audit row");
    assert_eq!(audit.get::<String, _>("status"), "SUCCESS");
    match &first_outcome {
        ProcessOutcome::Succeeded => {
            assert_eq!(audit.get::<i32, _>("attempt"), 1);
            assert_eq!(audit.get::<String, _>("runner_id"), "stale-runner-a");
        }
        ProcessOutcome::LeaseLost => {
            assert_eq!(audit.get::<i32, _>("attempt"), 2);
            assert_eq!(audit.get::<String, _>("runner_id"), "stale-runner-b");
        }
        _ => unreachable!(),
    }
    assert!(audit.get::<Option<String>, _>("error_code").is_none());
    rows.cleanup().await;
}
