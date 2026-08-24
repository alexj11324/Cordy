use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use cordy_db::queries::{agent, runtime};
use cordy_events::{Bus, Event};
use cordy_service::task_service::TaskService;
use serde_json::json;
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::runtime_liveness::LivenessStore;

const STALE_SECONDS: f64 = 150.0;
const OFFLINE_BATCH: i32 = 500;
const RECONNECT_BATCH: i32 = 500;
const QUEUED_TTL_SECONDS: f64 = 2.0 * 3600.0;
const QUEUED_BATCH: i32 = 500;
const DISPATCH_TIMEOUT_SECONDS: f64 = 300.0;
const RUNNING_TIMEOUT_SECONDS: f64 = 9000.0;
const RECOVERY_BATCH: i32 = 100;
const CHAT_FINALIZE_GRACE_SECONDS: f64 = 60.0;
const CHAT_FINALIZE_BATCH: i32 = 100;
const OFFLINE_RUNTIME_TTL_SECONDS: f64 = 7.0 * 24.0 * 3600.0;
const GC_BATCH: i32 = 100;
const GC_BLOCKED_LIMIT: i32 = 1000;
const GC_TICK_TIMEOUT: Duration = Duration::from_secs(15);
const GC_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_RECONNECT_GRACE: Duration = Duration::from_secs(3 * 60 * 60);
pub const MINIMUM_RECONNECT_GRACE: Duration = Duration::from_secs(STALE_SECONDS as u64);

#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeTaskSweepReport {
    pub runtimes_offline: usize,
    pub tasks_failed: usize,
    pub queued_expired: usize,
    pub recoveries_replayed: usize,
    pub recoveries_exhausted: usize,
    pub chats_finalized: usize,
    pub runtimes_gc_deleted: usize,
}

pub struct RuntimeTaskSweeper {
    pool: PgPool,
    liveness: Arc<dyn LivenessStore>,
    tasks: Arc<TaskService>,
    bus: Arc<Bus>,
    metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    reconnect_grace: Duration,
}

impl RuntimeTaskSweeper {
    pub fn new(
        pool: PgPool,
        liveness: Arc<dyn LivenessStore>,
        tasks: Arc<TaskService>,
        bus: Arc<Bus>,
        metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
        reconnect_grace: Duration,
    ) -> Self {
        Self {
            pool,
            liveness,
            tasks,
            bus,
            metrics,
            reconnect_grace: reconnect_grace.max(MINIMUM_RECONNECT_GRACE),
        }
    }

    pub async fn gc_once(&self) -> usize {
        match tokio::time::timeout(GC_TICK_TIMEOUT, self.gc_with_budget()).await {
            Ok(deleted) => deleted,
            Err(_) => {
                tracing::info!("runtime GC: tick budget exhausted");
                0
            }
        }
    }

    async fn gc_with_budget(&self) -> usize {
        match tokio::time::timeout(
            GC_OPERATION_TIMEOUT,
            runtime::count_stale_offline_runtimes_blocked_by_tasks(
                &self.pool,
                OFFLINE_RUNTIME_TTL_SECONDS,
                GC_BLOCKED_LIMIT,
            ),
        )
        .await
        {
            Ok(Ok(Some(count))) => {
                if let Some(metrics) = &self.metrics {
                    metrics.set_runtime_gc_blocked(count);
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "runtime GC: count blocked runtimes failed");
                if let Some(metrics) = &self.metrics {
                    metrics.record_runtime_gc_blocked_observation_failed();
                }
            }
            Err(_) => {
                tracing::warn!("runtime GC: count blocked runtimes timed out");
                if let Some(metrics) = &self.metrics {
                    metrics.record_runtime_gc_blocked_observation_failed();
                }
            }
        }
        let candidates = match tokio::time::timeout(
            GC_OPERATION_TIMEOUT,
            runtime::list_stale_offline_runtime_gc_candidates(
                &self.pool,
                OFFLINE_RUNTIME_TTL_SECONDS,
                GC_BATCH,
            ),
        )
        .await
        {
            Ok(Ok(rows)) => rows.into_iter().flatten().collect::<Vec<_>>(),
            Ok(Err(error)) => {
                tracing::warn!(%error, "runtime GC: list candidates failed");
                if let Some(metrics) = &self.metrics {
                    metrics.record_runtime_gc_failed();
                }
                return 0;
            }
            Err(_) => {
                tracing::warn!("runtime GC: list candidates timed out");
                if let Some(metrics) = &self.metrics {
                    metrics.record_runtime_gc_failed();
                }
                return 0;
            }
        };
        let mut deleted = 0;
        let mut workspaces = HashSet::new();
        for runtime_id in candidates {
            match tokio::time::timeout(GC_OPERATION_TIMEOUT, self.gc_runtime(runtime_id)).await {
                Ok(Ok(Some(workspace_id))) => {
                    deleted += 1;
                    workspaces.insert(workspace_id);
                    if let Some(metrics) = &self.metrics {
                        metrics.record_runtime_gc_deleted();
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %runtime_id, "runtime GC: delete candidate failed");
                    if let Some(metrics) = &self.metrics {
                        metrics.record_runtime_gc_failed();
                    }
                }
                Err(_) => {
                    tracing::warn!(%runtime_id, "runtime GC: candidate timed out");
                    if let Some(metrics) = &self.metrics {
                        metrics.record_runtime_gc_failed();
                    }
                }
            }
        }
        for workspace_id in workspaces {
            self.bus.publish(&Event {
                event_type: cordy_protocol::EVENT_DAEMON_REGISTER.into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "system".into(),
                payload: json!({"action": "runtime_gc"}),
                ..Default::default()
            });
        }
        deleted
    }

    async fn gc_runtime(&self, runtime_id: uuid::Uuid) -> anyhow::Result<Option<uuid::Uuid>> {
        let mut tx = self.pool.begin().await?;
        let Some(agent_runtime) = runtime::lock_agent_runtime(&mut *tx, runtime_id).await? else {
            return Ok(None);
        };
        let eligible = runtime::is_agent_runtime_eligible_for_gc(
            &mut *tx,
            runtime_id,
            OFFLINE_RUNTIME_TTL_SECONDS,
        )
        .await?;
        if !matches!(eligible, Some(true)) {
            return Ok(None);
        }
        let undrained = runtime::count_undrained_tasks_by_runtime_or_agent(
            &mut *tx,
            vec![runtime_id],
            Vec::new(),
        )
        .await?;
        let undrained = match undrained {
            Some(value) => value,
            None => 0,
        };
        if undrained > 0 {
            return Ok(None);
        }
        runtime::unbind_tasks_from_runtime(&mut *tx, runtime_id).await?;
        let remaining = runtime::count_tasks_by_runtime(&mut *tx, runtime_id).await?;
        let remaining = match remaining {
            Some(value) => value,
            None => 0,
        };
        anyhow::ensure!(
            remaining == 0,
            "task history still references runtime after detach: {remaining}"
        );
        runtime::delete_agent_runtime(&mut *tx, runtime_id).await?;
        tx.commit().await?;
        Ok(Some(agent_runtime.workspace_id))
    }

    /// Runs the ordered non-GC stages from Go's runtime sweeper. Each stage is
    /// failure-isolated so a transient query error does not starve later repair
    /// stages on the same tick.
    pub async fn run_once(&self) -> RuntimeTaskSweepReport {
        let mut report = RuntimeTaskSweepReport::default();
        report.runtimes_offline = self.sweep_stale_runtimes().await;

        let grace = self.reconnect_grace.as_secs_f64();
        match runtime::fail_tasks_for_offline_runtimes(&self.pool, grace, OFFLINE_BATCH).await {
            Ok(failed) => {
                report.tasks_failed += failed.len();
                self.tasks.handle_failed_tasks(&failed).await;
            }
            Err(error) => tracing::warn!(%error, "runtime sweeper: fail offline tasks failed"),
        }
        match agent::fail_expired_runtime_reconnect_retries(
            &self.pool,
            grace,
            STALE_SECONDS,
            RECONNECT_BATCH,
        )
        .await
        {
            Ok(failed) => {
                report.tasks_failed += failed.len();
                self.tasks.handle_failed_tasks(&failed).await;
            }
            Err(error) => {
                tracing::warn!(%error, "runtime sweeper: expire reconnect retries failed")
            }
        }
        match agent::fail_stale_tasks(
            &self.pool,
            DISPATCH_TIMEOUT_SECONDS,
            STALE_SECONDS,
            grace,
            RUNNING_TIMEOUT_SECONDS,
        )
        .await
        {
            Ok(failed) => {
                report.tasks_failed += failed.len();
                self.tasks.handle_failed_tasks(&failed).await;
            }
            Err(error) => tracing::warn!(%error, "runtime sweeper: fail stale tasks failed"),
        }
        match agent::expire_stale_queued_tasks(&self.pool, QUEUED_TTL_SECONDS, QUEUED_BATCH).await {
            Ok(failed) => {
                report.queued_expired = failed.len();
                self.tasks.capture_queued_expired_tasks(&failed).await;
                self.tasks.handle_failed_tasks(&failed).await;
            }
            Err(error) => tracing::warn!(%error, "runtime sweeper: expire queued tasks failed"),
        }
        match self
            .tasks
            .recover_pending_delegated_failures(RECOVERY_BATCH)
            .await
        {
            Ok(result) => {
                report.recoveries_replayed = match usize::try_from(result.replayed) {
                    Ok(value) => value,
                    Err(_) => 0,
                };
                report.recoveries_exhausted = match usize::try_from(result.exhausted) {
                    Ok(value) => value,
                    Err(_) => 0,
                };
            }
            Err(error) => tracing::warn!(%error, "delegated failure recovery sweep failed"),
        }
        match agent::list_chat_finalize_deferred_expired(
            &self.pool,
            CHAT_FINALIZE_GRACE_SECONDS,
            CHAT_FINALIZE_BATCH,
        )
        .await
        {
            Ok(rows) => {
                report.chats_finalized = rows.len();
                for task in rows {
                    self.tasks.finalize_deferred_cancelled_chat(task.id).await;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "runtime sweeper: list deferred chat finalizations failed")
            }
        }
        report
    }

    pub async fn run_full_once(&self) -> RuntimeTaskSweepReport {
        let mut report = self.run_once().await;
        report.runtimes_gc_deleted = self.gc_once().await;
        report
    }

    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> RuntimeSweeperRuntime {
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        RuntimeSweeperRuntime {
            cancel,
            task: Some(task),
        }
    }

    async fn run(&self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(DEFAULT_SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => { self.run_full_once().await; }
            }
        }
    }

    async fn sweep_stale_runtimes(&self) -> usize {
        let candidates =
            match runtime::select_stale_online_runtimes(&self.pool, STALE_SECONDS).await {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!(%error, "runtime sweeper: list stale runtimes failed");
                    return 0;
                }
            };
        let ids = candidates
            .iter()
            .filter_map(|row| row.id)
            .collect::<Vec<_>>();
        let id_strings = ids.iter().map(ToString::to_string).collect::<Vec<_>>();
        let (alive, ok) = self.liveness.is_alive_batch(&id_strings).await;
        let to_offline = if ok {
            ids.into_iter()
                .filter(|id| !alive.get(&id.to_string()).copied().unwrap_or(false))
                .collect()
        } else {
            ids
        };
        if to_offline.is_empty() {
            return 0;
        }
        let rows =
            match runtime::mark_runtimes_offline_by_i_ds(&self.pool, to_offline, STALE_SECONDS)
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!(%error, "runtime sweeper: mark offline failed");
                    return 0;
                }
            };
        let mut workspaces = HashSet::new();
        for row in &rows {
            if let Some(id) = row.id {
                self.liveness.forget(&id.to_string()).await;
            }
            if let Some(workspace_id) = row.workspace_id {
                workspaces.insert(workspace_id);
            }
        }
        for workspace_id in workspaces {
            self.bus.publish(&Event {
                event_type: cordy_protocol::EVENT_DAEMON_REGISTER.into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "system".into(),
                payload: json!({"action": "stale_sweep"}),
                ..Default::default()
            });
        }
        rows.len()
    }
}

pub struct RuntimeSweeperRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl RuntimeSweeperRuntime {
    pub async fn shutdown(mut self) -> RuntimeSweeperShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return RuntimeSweeperShutdownOutcome::Panicked;
        };
        match tokio::time::timeout(DEFAULT_SHUTDOWN_TIMEOUT, &mut task).await {
            Ok(Ok(())) => RuntimeSweeperShutdownOutcome::Stopped,
            Ok(Err(_)) => RuntimeSweeperShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                RuntimeSweeperShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for RuntimeSweeperRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSweeperShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}
