use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cordy_db::queries::{agent, runtime};
use cordy_events::{Bus, Event};
use cordy_service::task_service::TaskService;
use serde_json::json;
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::runtime_liveness::LivenessStore;

const STALE_THRESHOLD: Duration = Duration::from_secs(150);
const STALE_RUNTIME_BATCH: i32 = 500;
const OFFLINE_BATCH: i32 = 500;
const RECONNECT_BATCH: i32 = 500;
const STALE_TASK_BATCH: i32 = 500;
const QUEUED_TTL: Duration = Duration::from_secs(2 * 60 * 60);
const QUEUED_BATCH: i32 = 500;
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(300);
const RUNNING_TIMEOUT: Duration = Duration::from_secs(9_000);
const RECOVERY_BATCH: i32 = 100;
const CHAT_FINALIZE_GRACE: Duration = Duration::from_secs(60);
const CHAT_FINALIZE_BATCH: i32 = 100;
const OFFLINE_RUNTIME_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const GC_BATCH: i32 = 100;
const GC_BLOCKED_LIMIT: i32 = 1000;
const GC_TICK_TIMEOUT: Duration = Duration::from_secs(15);
const GC_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_RECONNECT_GRACE: Duration = Duration::from_secs(3 * 60 * 60);
pub const MINIMUM_RECONNECT_GRACE: Duration = STALE_THRESHOLD;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

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
    clock: Arc<dyn Clock>,
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
            clock: Arc::new(SystemClock),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub async fn gc_once(&self) -> usize {
        self.gc_once_at(self.clock.now()).await
    }

    async fn gc_once_at(&self, now: DateTime<Utc>) -> usize {
        let stale_before = cutoff(now, OFFLINE_RUNTIME_TTL);
        match tokio::time::timeout(GC_TICK_TIMEOUT, self.gc_with_budget(stale_before)).await {
            Ok(deleted) => deleted,
            Err(_) => {
                tracing::info!("runtime GC: tick budget exhausted");
                0
            }
        }
    }

    async fn gc_with_budget(&self, stale_before: DateTime<Utc>) -> usize {
        match tokio::time::timeout(
            GC_OPERATION_TIMEOUT,
            runtime::count_stale_offline_runtimes_blocked_by_tasks(
                &self.pool,
                stale_before,
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
            runtime::list_stale_offline_runtime_gc_candidates(&self.pool, stale_before, GC_BATCH),
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
            match tokio::time::timeout(
                GC_OPERATION_TIMEOUT,
                self.gc_runtime(runtime_id, stale_before),
            )
            .await
            {
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

    async fn gc_runtime(
        &self,
        runtime_id: uuid::Uuid,
        stale_before: DateTime<Utc>,
    ) -> anyhow::Result<Option<uuid::Uuid>> {
        let mut tx = self.pool.begin().await?;
        let Some(agent_runtime) = runtime::lock_agent_runtime(&mut *tx, runtime_id).await? else {
            return Ok(None);
        };
        let eligible =
            runtime::is_agent_runtime_eligible_for_gc(&mut *tx, runtime_id, stale_before).await?;
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
        self.run_once_at(self.clock.now()).await
    }

    async fn run_once_at(&self, now: DateTime<Utc>) -> RuntimeTaskSweepReport {
        let mut report = RuntimeTaskSweepReport::default();
        let stale_before = cutoff(now, STALE_THRESHOLD);
        report.runtimes_offline = self.sweep_stale_runtimes(stale_before).await;

        let reconnect_before = cutoff(now, self.reconnect_grace);
        match runtime::fail_tasks_for_offline_runtimes(&self.pool, reconnect_before, OFFLINE_BATCH)
            .await
        {
            Ok(failed) => {
                report.tasks_failed += failed.len();
                self.tasks.handle_failed_tasks(&failed).await;
            }
            Err(error) => tracing::warn!(%error, "runtime sweeper: fail offline tasks failed"),
        }
        match agent::fail_expired_runtime_reconnect_retries(
            &self.pool,
            reconnect_before,
            stale_before,
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
            cutoff(now, DISPATCH_TIMEOUT),
            now,
            stale_before,
            reconnect_before,
            cutoff(now, RUNNING_TIMEOUT),
            STALE_TASK_BATCH,
        )
        .await
        {
            Ok(failed) => {
                report.tasks_failed += failed.len();
                self.tasks.capture_lease_expired_tasks(&failed).await;
                self.tasks.handle_failed_tasks(&failed).await;
            }
            Err(error) => tracing::warn!(%error, "runtime sweeper: fail stale tasks failed"),
        }
        match agent::expire_stale_queued_tasks(&self.pool, cutoff(now, QUEUED_TTL), QUEUED_BATCH)
            .await
        {
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
            cutoff(now, CHAT_FINALIZE_GRACE),
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
        let now = self.clock.now();
        let mut report = self.run_once_at(now).await;
        report.runtimes_gc_deleted = self.gc_once_at(now).await;
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

    async fn sweep_stale_runtimes(&self, stale_before: DateTime<Utc>) -> usize {
        let candidates = match runtime::select_stale_online_runtimes(
            &self.pool,
            stale_before,
            STALE_RUNTIME_BATCH,
        )
        .await
        {
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
        let rows = match runtime::mark_runtimes_offline_by_i_ds(
            &self.pool,
            to_offline,
            stale_before,
        )
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

fn cutoff(now: DateTime<Utc>, age: Duration) -> DateTime<Utc> {
    now - chrono::Duration::from_std(age).expect("runtime sweep duration fits chrono")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use cordy_db::dbid::new_v7;
    use cordy_db::models::AgentRuntime;
    use cordy_events::Event;
    use uuid::Uuid;

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    struct RuntimeRows {
        pool: PgPool,
        workspace_id: uuid::Uuid,
    }

    impl RuntimeRows {
        async fn required() -> Self {
            let url = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL is required for runtime sweeper contracts");
            let pool = PgPool::connect(&url)
                .await
                .expect("runtime sweeper contract requires a reachable migrated PostgreSQL");
            let workspace_id = new_v7();
            sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, $2, $3)")
                .bind(workspace_id)
                .bind("Rust runtime sweeper contract")
                .bind(format!("rust-sweeper-{workspace_id}"))
                .execute(&pool)
                .await
                .expect("insert runtime sweeper workspace");
            Self { pool, workspace_id }
        }

        async fn runtime(&self, suffix: &str, status: &str, age: Duration) -> AgentRuntime {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO agent_runtime \
                 (id, workspace_id, daemon_id, name, runtime_mode, provider, status, last_seen_at) \
                 VALUES ($1, $2, $3, $4, 'local', $5, $6, now() - $7::interval)",
            )
            .bind(id)
            .bind(self.workspace_id)
            .bind(format!("sweeper-{suffix}"))
            .bind(format!("Sweeper {suffix}"))
            .bind(format!("provider-{suffix}"))
            .bind(status)
            .bind(format!("{} seconds", age.as_secs()))
            .execute(&self.pool)
            .await
            .expect("insert runtime sweeper runtime");
            runtime::get_agent_runtime(&self.pool, id)
                .await
                .expect("read runtime sweeper runtime")
                .expect("runtime sweeper runtime exists")
        }

        async fn status(&self, id: uuid::Uuid) -> String {
            sqlx::query_scalar("SELECT status FROM agent_runtime WHERE id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .expect("read runtime sweeper status")
        }

        async fn cleanup(&self) {
            sqlx::query("DELETE FROM workspace WHERE id = $1")
                .bind(self.workspace_id)
                .execute(&self.pool)
                .await
                .expect("clean runtime sweeper workspace");
        }
    }

    impl Drop for RuntimeRows {
        fn drop(&mut self) {
            let pool = self.pool.clone();
            let workspace_id = self.workspace_id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = sqlx::query("DELETE FROM workspace WHERE id = $1")
                        .bind(workspace_id)
                        .execute(&pool)
                        .await;
                });
            }
        }
    }

    struct TestLiveness {
        available: bool,
        alive: HashSet<String>,
        forgotten: Arc<Mutex<Vec<String>>>,
        race_id: Option<uuid::Uuid>,
        pool: Option<PgPool>,
    }

    #[async_trait]
    impl LivenessStore for TestLiveness {
        fn available(&self) -> bool {
            self.available
        }

        async fn touch(&self, _: &str, _: Duration) -> anyhow::Result<()> {
            Ok(())
        }

        async fn is_alive_batch(&self, runtime_ids: &[String]) -> (HashMap<String, bool>, bool) {
            if let (Some(race_id), Some(pool)) = (self.race_id, self.pool.clone()) {
                if runtime_ids.iter().any(|id| id == &race_id.to_string()) {
                    sqlx::query("UPDATE agent_runtime SET status = 'offline' WHERE id = $1")
                        .bind(race_id)
                        .execute(&pool)
                        .await
                        .expect("force runtime stale-sweep race");
                }
            }
            if !self.available {
                return (HashMap::new(), false);
            }
            (
                runtime_ids
                    .iter()
                    .map(|id| (id.clone(), self.alive.contains(id)))
                    .collect(),
                true,
            )
        }

        async fn forget(&self, runtime_id: &str) {
            self.forgotten
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(runtime_id.to_string());
        }
    }

    #[test]
    fn production_bounds_match_the_go_sweeper_contract() {
        assert_eq!(STALE_THRESHOLD, Duration::from_secs(150));
        assert_eq!(STALE_RUNTIME_BATCH, 500);
        assert_eq!(OFFLINE_BATCH, 500);
        assert_eq!(RECONNECT_BATCH, 500);
        assert_eq!(STALE_TASK_BATCH, 500);
        assert_eq!(QUEUED_BATCH, 500);
        assert_eq!(RECOVERY_BATCH, 100);
        assert_eq!(CHAT_FINALIZE_BATCH, 100);
        assert_eq!(GC_BATCH, 100);
        assert_eq!(GC_BLOCKED_LIMIT, 1_000);
    }

    #[test]
    fn one_injected_clock_snapshot_derives_all_cutoffs() {
        let now = DateTime::parse_from_rfc3339("2026-08-24T04:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock: Arc<dyn Clock> = Arc::new(FixedClock(now));

        assert_eq!(clock.now(), now);
        assert_eq!(
            cutoff(now, STALE_THRESHOLD),
            DateTime::parse_from_rfc3339("2026-08-24T04:27:30Z")
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(
            cutoff(now, DEFAULT_RECONNECT_GRACE),
            DateTime::parse_from_rfc3339("2026-08-24T01:30:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[tokio::test]
    async fn production_stale_sweep_filters_liveness_and_publishes_one_workspace_event() {
        let rows = RuntimeRows::required().await;
        let dead = rows.runtime("dead", "online", Duration::from_secs(300)).await;
        let alive = rows.runtime("alive", "online", Duration::from_secs(300)).await;
        let fresh = rows.runtime("fresh", "online", Duration::from_secs(30)).await;
        let already_offline = rows
            .runtime("offline", "offline", Duration::from_secs(300))
            .await;
        let forgotten = Arc::new(Mutex::new(Vec::new()));
        let liveness = Arc::new(TestLiveness {
            available: true,
            alive: HashSet::from([alive.id.to_string()]),
            forgotten: forgotten.clone(),
            race_id: None,
            pool: None,
        });
        let bus = Arc::new(Bus::new());
        let events = Arc::new(Mutex::new(Vec::<Event>::new()));
        {
            let events = events.clone();
            bus.subscribe(cordy_protocol::EVENT_DAEMON_REGISTER, move |event| {
                events
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(event.clone());
            });
        }
        let tasks = Arc::new(TaskService::new(rows.pool.clone(), bus.clone()));
        let sweeper = RuntimeTaskSweeper::new(
            rows.pool.clone(),
            liveness,
            tasks,
            bus,
            None,
            DEFAULT_RECONNECT_GRACE,
        );
        let stale_before = Utc::now() - chrono::Duration::seconds(150);
        assert_eq!(sweeper.sweep_stale_runtimes(stale_before).await, 1);
        assert_eq!(rows.status(dead.id).await, "offline");
        assert_eq!(rows.status(alive.id).await, "online");
        assert_eq!(rows.status(fresh.id).await, "online");
        assert_eq!(rows.status(already_offline.id).await, "offline");
        assert_eq!(
            forgotten
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            [dead.id.to_string()]
        );
        let events = events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].workspace_id, rows.workspace_id.to_string());
        assert_eq!(events[0].event_type, cordy_protocol::EVENT_DAEMON_REGISTER);
        assert_eq!(events[0].payload, serde_json::json!({"action": "stale_sweep"}));
        drop(events);
        sqlx::query("UPDATE agent_runtime SET status = 'offline' WHERE id = $1")
            .bind(alive.id)
            .execute(&rows.pool)
            .await
            .expect("isolate later sweeper cases");

        let unavailable = rows
            .runtime("unavailable", "online", Duration::from_secs(300))
            .await;
        let unavailable_forgotten = Arc::new(Mutex::new(Vec::new()));
        let unavailable_bus = Arc::new(Bus::new());
        let unavailable_sweeper = RuntimeTaskSweeper::new(
            rows.pool.clone(),
            Arc::new(TestLiveness {
                available: false,
                alive: HashSet::new(),
                forgotten: unavailable_forgotten.clone(),
                race_id: None,
                pool: None,
            }),
            Arc::new(TaskService::new(rows.pool.clone(), unavailable_bus.clone())),
            unavailable_bus,
            None,
            DEFAULT_RECONNECT_GRACE,
        );
        assert_eq!(
            unavailable_sweeper
                .sweep_stale_runtimes(stale_before)
                .await,
            1
        );
        assert_eq!(rows.status(unavailable.id).await, "offline");
        assert_eq!(
            unavailable_forgotten
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            [unavailable.id.to_string()]
        );

        let raced = rows.runtime("raced", "online", Duration::from_secs(300)).await;
        let race_forgotten = Arc::new(Mutex::new(Vec::new()));
        let race_bus = Arc::new(Bus::new());
        let race_sweeper = RuntimeTaskSweeper::new(
            rows.pool.clone(),
            Arc::new(TestLiveness {
                available: true,
                alive: HashSet::new(),
                forgotten: race_forgotten,
                race_id: Some(raced.id),
                pool: Some(rows.pool.clone()),
            }),
            Arc::new(TaskService::new(rows.pool.clone(), race_bus.clone())),
            race_bus,
            None,
            DEFAULT_RECONNECT_GRACE,
        );
        assert_eq!(
            race_sweeper.sweep_stale_runtimes(stale_before).await,
            0
        );
        assert_eq!(rows.status(raced.id).await, "offline");
        rows.cleanup().await;
    }

    struct RecoveryRows {
        pool: PgPool,
        workspace_id: Uuid,
        user_id: Uuid,
        old_agent_id: Uuid,
        grace_agent_id: Uuid,
        healthy_agent_id: Uuid,
        old_runtime_id: Uuid,
        grace_runtime_id: Uuid,
        healthy_runtime_id: Uuid,
        active_task_ids: Vec<Uuid>,
        grace_task_id: Uuid,
        offline_retry_id: Uuid,
        healthy_retry_id: Uuid,
        unrelated_retry_id: Uuid,
    }

    impl RecoveryRows {
        async fn required() -> anyhow::Result<Self> {
            let url = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL is required for offline task recovery contracts");
            let pool = PgPool::connect(&url).await?;
            let workspace_id = new_v7();
            sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, $2, $3)")
                .bind(workspace_id)
                .bind("Rust offline task recovery contract")
                .bind(format!("rust-recovery-{workspace_id}"))
                .execute(&pool)
                .await?;
            let suffix = workspace_id.simple().to_string();
            let user_id: Uuid = sqlx::query_scalar(
                "INSERT INTO \"user\" (name, email) VALUES ($1, $2) RETURNING id",
            )
            .bind("offline recovery contract user")
            .bind(format!("offline-recovery-{suffix}@example.test"))
            .fetch_one(&pool)
            .await?;
            sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'owner')")
                .bind(workspace_id)
                .bind(user_id)
                .execute(&pool)
                .await?;

            let old_runtime_id = Self::runtime(&pool, workspace_id, "old", "offline", "4 hours").await?;
            let grace_runtime_id =
                Self::runtime(&pool, workspace_id, "grace", "offline", "10 minutes").await?;
            let healthy_runtime_id = Self::runtime(&pool, workspace_id, "healthy", "online", "1 minute").await?;
            let old_agent_id = Self::agent(&pool, workspace_id, user_id, old_runtime_id, "old").await?;
            let grace_agent_id =
                Self::agent(&pool, workspace_id, user_id, grace_runtime_id, "grace").await?;
            let healthy_agent_id =
                Self::agent(&pool, workspace_id, user_id, healthy_runtime_id, "healthy").await?;

            let mut next_number = 1;
            let mut active_task_ids = Vec::new();
            for status in ["dispatched", "running", "waiting_local_directory"] {
                let issue_id = Self::issue(&pool, workspace_id, user_id, old_agent_id, next_number).await?;
                next_number += 1;
                let wait_reason = (status == "waiting_local_directory").then_some("local directory busy");
                active_task_ids.push(
                    Self::task(
                        &pool,
                        old_agent_id,
                        old_runtime_id,
                        issue_id,
                        status,
                        None,
                        None,
                        None,
                        wait_reason,
                        None,
                    )
                    .await?,
                );
            }
            let grace_issue = Self::issue(&pool, workspace_id, user_id, grace_agent_id, next_number).await?;
            next_number += 1;
            let grace_task_id = Self::task(
                &pool,
                grace_agent_id,
                grace_runtime_id,
                grace_issue,
                "running",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;

            let offline_retry_issue = Self::issue(&pool, workspace_id, user_id, old_agent_id, next_number).await?;
            next_number += 1;
            let offline_parent = Self::task(
                &pool,
                old_agent_id,
                old_runtime_id,
                offline_retry_issue,
                "failed",
                None,
                None,
                Some("runtime_offline"),
                None,
                None,
            )
            .await?;
            let offline_retry_id = Self::task(
                &pool,
                old_agent_id,
                old_runtime_id,
                offline_retry_issue,
                "deferred",
                Some(offline_parent),
                Some(offline_parent),
                None,
                None,
                Some(Utc::now() - chrono::Duration::hours(4)),
            )
            .await?;

            let healthy_retry_issue = Self::issue(&pool, workspace_id, user_id, healthy_agent_id, next_number).await?;
            next_number += 1;
            let healthy_parent = Self::task(
                &pool,
                healthy_agent_id,
                healthy_runtime_id,
                healthy_retry_issue,
                "failed",
                None,
                None,
                Some("runtime_offline"),
                None,
                None,
            )
            .await?;
            let healthy_retry_id = Self::task(
                &pool,
                healthy_agent_id,
                healthy_runtime_id,
                healthy_retry_issue,
                "deferred",
                Some(healthy_parent),
                Some(healthy_parent),
                None,
                None,
                Some(Utc::now() - chrono::Duration::hours(4)),
            )
            .await?;

            let unrelated_retry_issue = Self::issue(&pool, workspace_id, user_id, old_agent_id, next_number).await?;
            let unrelated_parent = Self::task(
                &pool,
                old_agent_id,
                old_runtime_id,
                unrelated_retry_issue,
                "failed",
                None,
                None,
                Some("provider_auth"),
                None,
                None,
            )
            .await?;
            let unrelated_retry_id = Self::task(
                &pool,
                old_agent_id,
                old_runtime_id,
                unrelated_retry_issue,
                "deferred",
                Some(unrelated_parent),
                Some(unrelated_parent),
                None,
                None,
                Some(Utc::now() - chrono::Duration::hours(4)),
            )
            .await?;

            Ok(Self {
                pool,
                workspace_id,
                user_id,
                old_agent_id,
                grace_agent_id,
                healthy_agent_id,
                old_runtime_id,
                grace_runtime_id,
                healthy_runtime_id,
                active_task_ids,
                grace_task_id,
                offline_retry_id,
                healthy_retry_id,
                unrelated_retry_id,
            })
        }

        async fn runtime(
            pool: &PgPool,
            workspace_id: Uuid,
            suffix: &str,
            status: &str,
            age: &str,
        ) -> anyhow::Result<Uuid> {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO agent_runtime \
                 (id, workspace_id, daemon_id, name, runtime_mode, provider, status, last_seen_at) \
                 VALUES ($1, $2, $3, $4, 'local', $5, $6, now() - $7::interval)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(format!("recovery-{suffix}-{id}"))
            .bind(format!("Recovery {suffix}"))
            .bind(format!("recovery-{suffix}"))
            .bind(status)
            .bind(age)
            .execute(pool)
            .await?;
            Ok(id)
        }

        async fn agent(
            pool: &PgPool,
            workspace_id: Uuid,
            owner_id: Uuid,
            runtime_id: Uuid,
            suffix: &str,
        ) -> anyhow::Result<Uuid> {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO agent \
                 (id, workspace_id, name, runtime_mode, status, max_concurrent_tasks, owner_id, runtime_id) \
                 VALUES ($1, $2, $3, 'local', 'working', 6, $4, $5)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(format!("Recovery agent {suffix}"))
            .bind(owner_id)
            .bind(runtime_id)
            .execute(pool)
            .await?;
            Ok(id)
        }

        async fn issue(
            pool: &PgPool,
            workspace_id: Uuid,
            creator_id: Uuid,
            assignee_id: Uuid,
            number: i32,
        ) -> anyhow::Result<Uuid> {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO issue \
                 (id, workspace_id, title, status, priority, creator_type, creator_id, assignee_type, assignee_id, number, position) \
                 VALUES ($1, $2, $3, 'in_progress', 'none', 'member', $4, 'agent', $5, $6, -1) RETURNING id",
            )
            .bind(new_v7())
            .bind(workspace_id)
            .bind(format!("Recovery issue {number}"))
            .bind(creator_id)
            .bind(assignee_id)
            .bind(number)
            .fetch_one(pool)
            .await?;
            Ok(id)
        }

        #[allow(clippy::too_many_arguments)]
        async fn task(
            pool: &PgPool,
            agent_id: Uuid,
            runtime_id: Uuid,
            issue_id: Uuid,
            status: &str,
            parent_task_id: Option<Uuid>,
            retry_of_task_id: Option<Uuid>,
            failure_reason: Option<&str>,
            wait_reason: Option<&str>,
            fire_at: Option<chrono::DateTime<Utc>>,
        ) -> anyhow::Result<Uuid> {
            let task_id = new_v7();
            let active_at = (status == "dispatched" || status == "running")
                .then_some(Utc::now() - chrono::Duration::minutes(1));
            let started_at = (status == "running")
                .then_some(Utc::now() - chrono::Duration::minutes(1));
            let completed_at = (status == "failed")
                .then_some(Utc::now() - chrono::Duration::minutes(1));
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO agent_task_queue \
                 (id, agent_id, runtime_id, issue_id, status, priority, attempt, max_attempts, \
                  dispatched_at, started_at, completed_at, fire_at, parent_task_id, retry_of_task_id, failure_reason, wait_reason) \
                 VALUES ($1, $2, $3, $4, $5, 0, 1, 1, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id",
            )
            .bind(task_id)
            .bind(agent_id)
            .bind(runtime_id)
            .bind(issue_id)
            .bind(status)
            .bind(active_at)
            .bind(started_at)
            .bind(completed_at)
            .bind(fire_at)
            .bind(parent_task_id)
            .bind(retry_of_task_id)
            .bind(failure_reason)
            .bind(wait_reason)
            .fetch_one(pool)
            .await?;
            Ok(id)
        }

        async fn cleanup(&self) -> anyhow::Result<()> {
            sqlx::query("DELETE FROM workspace WHERE id = $1")
                .bind(self.workspace_id)
                .execute(&self.pool)
                .await?;
            sqlx::query("DELETE FROM \"user\" WHERE id = $1")
                .bind(self.user_id)
                .execute(&self.pool)
                .await?;
            Ok(())
        }

        async fn status(&self, id: Uuid) -> anyhow::Result<String> {
            Ok(sqlx::query_scalar("SELECT status FROM agent_task_queue WHERE id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?)
        }
    }

    impl Drop for RecoveryRows {
        fn drop(&mut self) {
            let pool = self.pool.clone();
            let workspace_id = self.workspace_id;
            let user_id = self.user_id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = sqlx::query("DELETE FROM workspace WHERE id = $1")
                        .bind(workspace_id)
                        .execute(&pool)
                        .await;
                    let _ = sqlx::query("DELETE FROM \"user\" WHERE id = $1")
                        .bind(user_id)
                        .execute(&pool)
                        .await;
                });
            }
        }
    }

    #[tokio::test]
    async fn production_offline_task_recovery_and_reconnect_retry_contract() {
        let rows = RecoveryRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for recovery contract");
        let result = async {
            let mut lock = rows.pool.begin().await?;
            sqlx::query("SELECT id FROM agent_task_queue WHERE id = $1 FOR UPDATE")
                .bind(rows.active_task_ids[0])
                .fetch_one(&mut *lock)
                .await?;
            let selected = runtime::fail_tasks_for_offline_runtimes(
                &rows.pool,
                Utc::now() - chrono::Duration::hours(3),
                1,
            )
            .await?;
            anyhow::ensure!(selected.len() == 1, "offline failure batch returned {} rows", selected.len());
            anyhow::ensure!(
                rows.active_task_ids.contains(&selected[0].id),
                "offline failure query selected an unrelated task"
            );
            anyhow::ensure!(
                selected[0].id != rows.active_task_ids[0],
                "FOR UPDATE SKIP LOCKED selected the held task"
            );
            lock.commit().await?;
            sqlx::query(
                "UPDATE agent_task_queue SET status = 'running', dispatched_at = now(), started_at = now(), \
                 completed_at = NULL, error = NULL, failure_reason = NULL, wait_reason = NULL WHERE id = $1",
            )
            .bind(selected[0].id)
            .execute(&rows.pool)
            .await?;

            let bus = Arc::new(Bus::new());
            let failed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
            {
                let failed_events = failed_events.clone();
                bus.subscribe(cordy_protocol::EVENT_TASK_FAILED, move |event| {
                    failed_events
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(event.clone());
                });
            }
            let issue_events = Arc::new(Mutex::new(Vec::<Event>::new()));
            {
                let issue_events = issue_events.clone();
                bus.subscribe(cordy_protocol::EVENT_ISSUE_UPDATED, move |event| {
                    issue_events
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(event.clone());
                });
            }
            let tasks = Arc::new(TaskService::new(rows.pool.clone(), bus.clone()));
            let liveness = Arc::new(TestLiveness {
                available: false,
                alive: HashSet::new(),
                forgotten: Arc::new(Mutex::new(Vec::new())),
                race_id: None,
                pool: None,
            });
            let sweeper = RuntimeTaskSweeper::new(
                rows.pool.clone(),
                liveness,
                tasks,
                bus,
                None,
                DEFAULT_RECONNECT_GRACE,
            )
            .with_clock(Arc::new(FixedClock(Utc::now())));
            let report = sweeper.run_once().await;
            anyhow::ensure!(
                report.tasks_failed == rows.active_task_ids.len() + 1,
                "tasks_failed report = {}, want {}",
                report.tasks_failed,
                rows.active_task_ids.len() + 1
            );
            for task_id in &rows.active_task_ids {
                anyhow::ensure!(rows.status(*task_id).await? == "failed", "active task did not fail");
                let (error, reason, completed, wait_reason): (Option<String>, Option<String>, Option<chrono::DateTime<Utc>>, Option<String>) = sqlx::query_as(
                    "SELECT error, failure_reason, completed_at, wait_reason FROM agent_task_queue WHERE id = $1",
                )
                .bind(task_id)
                .fetch_one(&rows.pool)
                .await?;
                anyhow::ensure!(error.as_deref() == Some("runtime went offline"), "offline error = {error:?}");
                anyhow::ensure!(reason.as_deref() == Some("runtime_offline"), "offline reason = {reason:?}");
                anyhow::ensure!(completed.is_some(), "offline task has no completion timestamp");
                anyhow::ensure!(wait_reason.is_none(), "offline waiter retained wait reason");
            }
            anyhow::ensure!(rows.status(rows.grace_task_id).await? == "running", "task inside reconnect grace was failed");
            anyhow::ensure!(rows.status(rows.offline_retry_id).await? == "failed", "expired offline retry was not terminalized");
            anyhow::ensure!(rows.status(rows.healthy_retry_id).await? == "deferred", "healthy runtime retry was expired");
            anyhow::ensure!(rows.status(rows.unrelated_retry_id).await? == "deferred", "unrelated retry lineage was expired");
            let retry_reason: Option<String> = sqlx::query_scalar(
                "SELECT failure_reason FROM agent_task_queue WHERE id = $1",
            )
            .bind(rows.offline_retry_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(
                retry_reason.as_deref() == Some("runtime_reconnect_timeout"),
                "retry failure reason = {retry_reason:?}"
            );
            let old_status: String = sqlx::query_scalar("SELECT status FROM agent WHERE id = $1")
                .bind(rows.old_agent_id)
                .fetch_one(&rows.pool)
                .await?;
            let grace_status: String = sqlx::query_scalar("SELECT status FROM agent WHERE id = $1")
                .bind(rows.grace_agent_id)
                .fetch_one(&rows.pool)
                .await?;
            anyhow::ensure!(old_status == "idle", "old agent status = {old_status}");
            anyhow::ensure!(grace_status == "working", "grace agent status = {grace_status}");
            let failed_events = failed_events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            anyhow::ensure!(
                failed_events.len() == rows.active_task_ids.len() + 1,
                "task failure events = {}, want {}",
                failed_events.len(),
                rows.active_task_ids.len() + 1
            );
            for event in failed_events.iter() {
                anyhow::ensure!(event.workspace_id == rows.workspace_id.to_string(), "failure event workspace mismatch");
                anyhow::ensure!(event.payload["failure_reason"].is_string(), "failure event omitted reason");
            }
            drop(failed_events);
            anyhow::ensure!(
                issue_events.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
                    >= rows.active_task_ids.len() + 1,
                "terminal failures did not reconcile issue events"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("offline task recovery contract failed");
        cleanup.expect("offline task recovery fixture cleanup failed");
    }

    #[tokio::test]
    async fn production_stale_and_queued_task_cleanup_contract() {
        let rows = RecoveryRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for cleanup contract");
        let result = async {
            sqlx::query(
                "UPDATE agent_task_queue AS task SET status = 'completed', completed_at = now(), \
                 error = NULL, failure_reason = NULL, wait_reason = NULL \
                 FROM issue WHERE task.issue_id = issue.id AND issue.workspace_id = $1",
            )
            .bind(rows.workspace_id)
            .execute(&rows.pool)
            .await?;
            sqlx::query("UPDATE agent SET status = 'idle' WHERE workspace_id = $1")
                .bind(rows.workspace_id)
                .execute(&rows.pool)
                .await?;

            let stale_runtime =
                RecoveryRows::runtime(&rows.pool, rows.workspace_id, "stale", "online", "2 hours")
                    .await?;
            let stale_agent =
                RecoveryRows::agent(&rows.pool, rows.workspace_id, rows.user_id, stale_runtime, "stale")
                    .await?;

            let stale_dispatch_issue =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, rows.healthy_agent_id, 101)
                    .await?;
            let stale_dispatch = RecoveryRows::task(
                &rows.pool,
                rows.healthy_agent_id,
                rows.healthy_runtime_id,
                stale_dispatch_issue,
                "dispatched",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            let leased_dispatch_issue =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, rows.healthy_agent_id, 102)
                    .await?;
            let leased_dispatch = RecoveryRows::task(
                &rows.pool,
                rows.healthy_agent_id,
                rows.healthy_runtime_id,
                leased_dispatch_issue,
                "dispatched",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            let fresh_running_issue =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, rows.healthy_agent_id, 103)
                    .await?;
            let fresh_running = RecoveryRows::task(
                &rows.pool,
                rows.healthy_agent_id,
                rows.healthy_runtime_id,
                fresh_running_issue,
                "running",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            let stale_running_issue =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, stale_agent, 104)
                    .await?;
            let stale_running = RecoveryRows::task(
                &rows.pool,
                stale_agent,
                stale_runtime,
                stale_running_issue,
                "running",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            let waiting_issue =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, stale_agent, 105)
                    .await?;
            let waiting = RecoveryRows::task(
                &rows.pool,
                stale_agent,
                stale_runtime,
                waiting_issue,
                "waiting_local_directory",
                None,
                None,
                None,
                Some("local directory busy"),
                None,
            )
            .await?;

            sqlx::query(
                "UPDATE agent_task_queue SET dispatched_at = now() - interval '10 minutes', \
                 prepare_lease_expires_at = NULL WHERE id = $1",
            )
            .bind(stale_dispatch)
            .execute(&rows.pool)
            .await?;
            sqlx::query(
                "UPDATE agent_task_queue SET dispatched_at = now() - interval '10 minutes', \
                 prepare_lease_expires_at = now() + interval '10 minutes' WHERE id = $1",
            )
            .bind(leased_dispatch)
            .execute(&rows.pool)
            .await?;
            sqlx::query("UPDATE agent_task_queue SET started_at = now() - interval '4 hours' WHERE id IN ($1, $2)")
                .bind(fresh_running)
                .bind(stale_running)
                .execute(&rows.pool)
                .await?;
            sqlx::query(
                "UPDATE agent_task_queue SET dispatched_at = now() - interval '4 hours', \
                 wait_reason = 'local directory busy' WHERE id = $1",
            )
            .bind(waiting)
            .execute(&rows.pool)
            .await?;

            let queued_issue_one =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, rows.healthy_agent_id, 106)
                    .await?;
            let queued_one = RecoveryRows::task(
                &rows.pool,
                rows.healthy_agent_id,
                rows.healthy_runtime_id,
                queued_issue_one,
                "queued",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            let queued_issue_two =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, rows.healthy_agent_id, 107)
                    .await?;
            let queued_two = RecoveryRows::task(
                &rows.pool,
                rows.healthy_agent_id,
                rows.healthy_runtime_id,
                queued_issue_two,
                "queued",
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            for id in [queued_one, queued_two] {
                sqlx::query("UPDATE agent_task_queue SET created_at = now() - interval '3 hours' WHERE id = $1")
                    .bind(id)
                    .execute(&rows.pool)
                    .await?;
            }

            let retry_issue =
                RecoveryRows::issue(&rows.pool, rows.workspace_id, rows.user_id, rows.old_agent_id, 108)
                    .await?;
            let retry_parent = RecoveryRows::task(
                &rows.pool,
                rows.old_agent_id,
                rows.old_runtime_id,
                retry_issue,
                "failed",
                None,
                None,
                Some("runtime_offline"),
                None,
                None,
            )
            .await?;
            let queued_retry = RecoveryRows::task(
                &rows.pool,
                rows.old_agent_id,
                rows.old_runtime_id,
                retry_issue,
                "queued",
                Some(retry_parent),
                Some(retry_parent),
                None,
                None,
                None,
            )
            .await?;
            sqlx::query("UPDATE agent_task_queue SET created_at = now() - interval '3 hours' WHERE id = $1")
                .bind(queued_retry)
                .execute(&rows.pool)
                .await?;

            let mut lock = rows.pool.begin().await?;
            sqlx::query("SELECT id FROM agent_task_queue WHERE id = $1 FOR UPDATE")
                .bind(queued_one)
                .fetch_one(&mut *lock)
                .await?;
            let selected = agent::expire_stale_queued_tasks(
                &rows.pool,
                Utc::now() - chrono::Duration::hours(2),
                1,
            )
            .await?;
            anyhow::ensure!(selected.len() == 1, "queued cleanup batch returned {} rows", selected.len());
            anyhow::ensure!(selected[0].id == queued_two, "SKIP LOCKED selected the held or unrelated queued row");
            lock.commit().await?;
            sqlx::query(
                "UPDATE agent_task_queue SET status = 'queued', completed_at = NULL, error = NULL, \
                 failure_reason = NULL, prepare_lease_expires_at = NULL WHERE id = $1",
            )
            .bind(queued_two)
            .execute(&rows.pool)
            .await?;

            let bus = Arc::new(Bus::new());
            let events = Arc::new(Mutex::new(Vec::<Event>::new()));
            {
                let events = events.clone();
                bus.subscribe_all(move |event| {
                    events
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(event.clone());
                });
            }
            let tasks = Arc::new(TaskService::new(rows.pool.clone(), bus.clone()));
            let liveness = Arc::new(TestLiveness {
                available: true,
                alive: HashSet::from([stale_runtime.to_string()]),
                forgotten: Arc::new(Mutex::new(Vec::new())),
                race_id: None,
                pool: None,
            });
            let sweeper = RuntimeTaskSweeper::new(
                rows.pool.clone(),
                liveness,
                tasks,
                bus,
                None,
                DEFAULT_RECONNECT_GRACE,
            )
            .with_clock(Arc::new(FixedClock(Utc::now())));
            let report = sweeper.run_once().await;
            anyhow::ensure!(report.runtimes_offline == 0, "alive stale runtime was marked offline");
            anyhow::ensure!(report.tasks_failed == 4, "stale/queued tasks failed = {}, want 4", report.tasks_failed);
            anyhow::ensure!(report.queued_expired == 2, "queued_expired = {}, want 2", report.queued_expired);

            for id in [stale_dispatch, stale_running] {
                let (status, reason, error, lease): (String, Option<String>, Option<String>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
                    "SELECT status, failure_reason, error, prepare_lease_expires_at FROM agent_task_queue WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&rows.pool)
                .await?;
                anyhow::ensure!(status == "failed", "stale task status = {status}");
                anyhow::ensure!(reason.as_deref() == Some("timeout"), "stale task reason = {reason:?}");
                anyhow::ensure!(error.as_deref() == Some("task timed out"), "stale task error = {error:?}");
                anyhow::ensure!(lease.is_none(), "stale task retained prepare lease");
            }
            anyhow::ensure!(rows.status(leased_dispatch).await? == "dispatched", "active prepare lease was ignored");
            anyhow::ensure!(rows.status(fresh_running).await? == "running", "fresh runtime running task was timed out");
            anyhow::ensure!(rows.status(waiting).await? == "waiting_local_directory", "directory waiter was timed out");
            for id in [queued_one, queued_two] {
                let (status, reason, error): (String, Option<String>, Option<String>) = sqlx::query_as(
                    "SELECT status, failure_reason, error FROM agent_task_queue WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&rows.pool)
                .await?;
                anyhow::ensure!(status == "failed", "queued task status = {status}");
                anyhow::ensure!(reason.as_deref() == Some("queued_expired"), "queued reason = {reason:?}");
                anyhow::ensure!(error.as_deref() == Some("task expired in queue"), "queued error = {error:?}");
            }
            anyhow::ensure!(rows.status(queued_retry).await? == "queued", "runtime_offline retry was expired by queue TTL");
            let task_events = events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .filter(|event| event.event_type == cordy_protocol::EVENT_TASK_FAILED)
                .cloned()
                .collect::<Vec<_>>();
            anyhow::ensure!(task_events.len() == 4, "task failure events = {}, want 4", task_events.len());
            anyhow::ensure!(
                task_events.iter().all(|event| event.workspace_id == rows.workspace_id.to_string()),
                "task failure event workspace mismatch"
            );
            let reasons = task_events
                .iter()
                .filter_map(|event| event.payload["failure_reason"].as_str())
                .collect::<Vec<_>>();
            anyhow::ensure!(reasons.iter().filter(|reason| **reason == "timeout").count() == 2, "timeout event count mismatch");
            anyhow::ensure!(
                reasons.iter().filter(|reason| **reason == "queued_expired").count() == 2,
                "queued_expired event count mismatch"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("stale and queued task cleanup contract failed");
        cleanup.expect("stale and queued task cleanup fixture cleanup failed");
    }

    struct GcRows {
        pool: PgPool,
        workspace_id: Uuid,
        user_id: Uuid,
        helper_agent_id: Uuid,
    }

    impl GcRows {
        async fn required() -> anyhow::Result<Self> {
            let url = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL is required for runtime GC contracts");
            let pool = PgPool::connect(&url).await?;
            let workspace_id = new_v7();
            sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, $2, $3)")
                .bind(workspace_id)
                .bind("Rust runtime GC contract")
                .bind(format!("rust-runtime-gc-{workspace_id}"))
                .execute(&pool)
                .await?;
            let suffix = workspace_id.simple().to_string();
            let user_id: Uuid = sqlx::query_scalar(
                "INSERT INTO \"user\" (name, email) VALUES ($1, $2) RETURNING id",
            )
            .bind("runtime GC contract user")
            .bind(format!("runtime-gc-{suffix}@example.test"))
            .fetch_one(&pool)
            .await?;
            sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'owner')")
                .bind(workspace_id)
                .bind(user_id)
                .execute(&pool)
                .await?;
            let helper_runtime_id =
                RecoveryRows::runtime(&pool, workspace_id, "gc-helper", "online", "1 minute")
                    .await?;
            let helper_agent_id =
                RecoveryRows::agent(&pool, workspace_id, user_id, helper_runtime_id, "gc-helper")
                    .await?;
            Ok(Self {
                pool,
                workspace_id,
                user_id,
                helper_agent_id,
            })
        }

        async fn runtime(&self, suffix: &str, status: &str, age: &str) -> anyhow::Result<Uuid> {
            RecoveryRows::runtime(&self.pool, self.workspace_id, suffix, status, age).await
        }

        async fn issue(&self, number: i32) -> anyhow::Result<Uuid> {
            RecoveryRows::issue(
                &self.pool,
                self.workspace_id,
                self.user_id,
                self.helper_agent_id,
                number,
            )
            .await
        }

        async fn task(
            &self,
            runtime_id: Uuid,
            issue_id: Uuid,
            status: &str,
        ) -> anyhow::Result<Uuid> {
            RecoveryRows::task(
                &self.pool,
                self.helper_agent_id,
                runtime_id,
                issue_id,
                status,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        }

        async fn cleanup(&self) -> anyhow::Result<()> {
            sqlx::query("UPDATE agent SET runtime_id = NULL WHERE workspace_id = $1")
                .bind(self.workspace_id)
                .execute(&self.pool)
                .await?;
            sqlx::query("DELETE FROM workspace WHERE id = $1")
                .bind(self.workspace_id)
                .execute(&self.pool)
                .await?;
            sqlx::query("DELETE FROM \"user\" WHERE id = $1")
                .bind(self.user_id)
                .execute(&self.pool)
                .await?;
            Ok(())
        }
    }

    impl Drop for GcRows {
        fn drop(&mut self) {
            let pool = self.pool.clone();
            let workspace_id = self.workspace_id;
            let user_id = self.user_id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = sqlx::query("UPDATE agent SET runtime_id = NULL WHERE workspace_id = $1")
                        .bind(workspace_id)
                        .execute(&pool)
                        .await;
                    let _ = sqlx::query("DELETE FROM workspace WHERE id = $1")
                        .bind(workspace_id)
                        .execute(&pool)
                        .await;
                    let _ = sqlx::query("DELETE FROM \"user\" WHERE id = $1")
                        .bind(user_id)
                        .execute(&pool)
                        .await;
                });
            }
        }
    }

    async fn mark_task_terminal(pool: &PgPool, task_id: Uuid, status: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE agent_task_queue SET status = $2, completed_at = now(), error = NULL, failure_reason = NULL WHERE id = $1",
        )
        .bind(task_id)
        .bind(status)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn owner_fenced_enqueue(
        pool: &PgPool,
        agent_id: Uuid,
        runtime_id: Uuid,
        issue_id: Uuid,
    ) -> anyhow::Result<Option<Uuid>> {
        Ok(sqlx::query_scalar(
            "INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority) \
             SELECT $1, $2, $3, 'queued', 0 \
             WHERE lock_task_owner_rows($1, $3, $2) \
             RETURNING id",
        )
        .bind(agent_id)
        .bind(runtime_id)
        .bind(issue_id)
        .fetch_optional(pool)
        .await?)
    }

    async fn wait_for_lock_wait(pool: &PgPool, pid: i32) -> anyhow::Result<()> {
        for _ in 0..500 {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND state = 'active' AND wait_event_type = 'Lock')",
            )
            .bind(pid)
            .fetch_one(pool)
            .await?;
            if waiting {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::bail!("backend {pid} never waited for the runtime row lock")
    }

    async fn wait_for_blocked_task_detach(pool: &PgPool) -> anyhow::Result<()> {
        for _ in 0..500 {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (\
                    SELECT 1 FROM pg_stat_activity \
                    WHERE pid <> pg_backend_pid() \
                      AND state = 'active' \
                      AND wait_event_type = 'Lock' \
                      AND query LIKE '%UPDATE agent_task_queue%' \
                      AND query LIKE '%completed_at IS NOT NULL%'\
                )",
            )
            .fetch_one(pool)
            .await?;
            if waiting {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        anyhow::bail!("runtime GC never reached the blocked terminal-task detach")
    }

    #[tokio::test]
    async fn production_runtime_gc_preserves_terminal_history_and_deduplicates_workspace_event() {
        let rows = GcRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for runtime GC contract");
        let result = async {
            let drainable_one = rows.runtime("drainable-one", "offline", "8 days").await?;
            let drainable_two = rows.runtime("drainable-two", "offline", "8 days").await?;
            let blocked = rows.runtime("blocked", "offline", "8 days").await?;
            let active_agent_runtime = rows.runtime("active-agent", "offline", "8 days").await?;
            let fresh = rows.runtime("fresh", "offline", "1 day").await?;
            let online = rows.runtime("online", "online", "1 minute").await?;

            let terminal_issue = rows.issue(601).await?;
            let completed = rows
                .task(drainable_one, terminal_issue, "completed")
                .await?;
            let failed_issue = rows.issue(602).await?;
            let failed = rows.task(drainable_one, failed_issue, "failed").await?;
            let cancelled_issue = rows.issue(603).await?;
            let cancelled = rows
                .task(drainable_one, cancelled_issue, "cancelled")
                .await?;
            for (task_id, status) in [(completed, "completed"), (failed, "failed"), (cancelled, "cancelled")] {
                mark_task_terminal(&rows.pool, task_id, status).await?;
            }
            sqlx::query(
                "INSERT INTO task_message (task_id, seq, type, content) VALUES ($1, 1, 'assistant', 'runtime GC preserves this transcript')",
            )
            .bind(completed)
            .execute(&rows.pool)
            .await?;
            sqlx::query(
                "INSERT INTO task_usage (task_id, provider, model, input_tokens, output_tokens) VALUES ($1, 'test', 'runtime-gc', 10, 20)",
            )
            .bind(completed)
            .execute(&rows.pool)
            .await?;
            sqlx::query(
                "INSERT INTO task_token (token_hash, task_id, agent_id, workspace_id, user_id, expires_at) VALUES ($1, $2, $3, $4, $5, now() + interval '1 hour')",
            )
            .bind(format!("runtime-gc-{completed}"))
            .bind(completed)
            .bind(rows.helper_agent_id)
            .bind(rows.workspace_id)
            .bind(rows.user_id)
            .execute(&rows.pool)
            .await?;

            let blocked_issue = rows.issue(604).await?;
            let blocked_task = rows
                .task(blocked, blocked_issue, "deferred")
                .await?;
            let active_agent = RecoveryRows::agent(
                &rows.pool,
                rows.workspace_id,
                rows.user_id,
                active_agent_runtime,
                "gc-active",
            )
            .await?;

            let bus = Arc::new(Bus::new());
            let events = Arc::new(Mutex::new(Vec::<Event>::new()));
            {
                let events = events.clone();
                bus.subscribe(cordy_protocol::EVENT_DAEMON_REGISTER, move |event| {
                    events
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(event.clone());
                });
            }
            let sweeper = RuntimeTaskSweeper::new(
                rows.pool.clone(),
                Arc::new(TestLiveness {
                    available: true,
                    alive: HashSet::new(),
                    forgotten: Arc::new(Mutex::new(Vec::new())),
                    race_id: None,
                    pool: None,
                }),
                Arc::new(TaskService::new(rows.pool.clone(), bus.clone())),
                bus,
                None,
                DEFAULT_RECONNECT_GRACE,
            )
            .with_clock(Arc::new(FixedClock(Utc::now())));
            let report = sweeper.run_full_once().await;
            anyhow::ensure!(
                report.runtimes_gc_deleted >= 2,
                "runtime GC deleted {} rows, want at least the two drainable candidates",
                report.runtimes_gc_deleted
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runtime WHERE id = ANY($1::uuid[])")
                    .bind(vec![drainable_one, drainable_two])
                    .fetch_one(&rows.pool)
                    .await?
                    == 0,
                "drainable runtimes still exist after full sweeper"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_task_queue WHERE runtime_id = $1")
                    .bind(drainable_one)
                    .fetch_one(&rows.pool)
                    .await?
                    == 0,
                "terminal history still references deleted runtime"
            );
            let detached: (i64, bool, bool, bool) = sqlx::query_as(
                "SELECT count(*), bool_and(runtime_id IS NULL), EXISTS (SELECT 1 FROM task_message WHERE task_id = $1), EXISTS (SELECT 1 FROM task_usage WHERE task_id = $1 AND provider = 'test') FROM agent_task_queue WHERE id IN ($1, $2, $3)",
            )
            .bind(completed)
            .bind(failed)
            .bind(cancelled)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(detached.0 == 3 && detached.1 && detached.2 && detached.3, "terminal history was not retained and detached: {detached:?}");
            let token_count: i64 = sqlx::query_scalar("SELECT count(*) FROM task_token WHERE task_id = $1")
                .bind(completed)
                .fetch_one(&rows.pool)
                .await?;
            anyhow::ensure!(token_count == 1, "task-scoped token was deleted with runtime");
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runtime WHERE id = $1")
                    .bind(blocked)
                    .fetch_one(&rows.pool)
                    .await?
                    == 1
                    && sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_task_queue WHERE id = $1 AND runtime_id = $2 AND completed_at IS NULL")
                        .bind(blocked_task)
                        .bind(blocked)
                        .fetch_one(&rows.pool)
                        .await?
                        == 1,
                "non-terminal task was not protected from GC"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runtime WHERE id = $1")
                    .bind(active_agent_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                    == 1
                    && sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent WHERE id = $1 AND runtime_id = $2")
                        .bind(active_agent)
                        .bind(active_agent_runtime)
                        .fetch_one(&rows.pool)
                        .await?
                        == 1,
                "runtime with a bound agent was deleted"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runtime WHERE id = ANY($1::uuid[])")
                    .bind(vec![fresh, online])
                    .fetch_one(&rows.pool)
                    .await?
                    == 2,
                "fresh or online runtime was incorrectly collected"
            );
            let events = events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let gc_events = events
                .iter()
                .filter(|event| {
                    event.workspace_id == rows.workspace_id.to_string()
                        && event.payload == serde_json::json!({"action": "runtime_gc"})
                })
                .collect::<Vec<_>>();
            anyhow::ensure!(gc_events.len() == 1, "runtime GC published {} workspace events, want one deduplicated event", gc_events.len());
            anyhow::ensure!(gc_events[0].workspace_id == rows.workspace_id.to_string(), "runtime GC event workspace mismatch");
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("runtime GC history/event contract failed");
        cleanup.expect("runtime GC fixture cleanup failed");
    }

    #[tokio::test]
    async fn production_runtime_gc_rechecks_all_task_and_agent_guards() {
        let rows = GcRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for runtime GC guards");
        let result = async {
            let blocked_before = runtime::count_stale_offline_runtimes_blocked_by_tasks(
                &rows.pool,
                Utc::now() - chrono::Duration::days(7),
                GC_BLOCKED_LIMIT,
            )
            .await?
            .unwrap_or_default();
            for (offset, status) in [
                "queued",
                "dispatched",
                "running",
                "waiting_local_directory",
                "deferred",
            ]
            .into_iter()
            .enumerate()
            {
                let runtime_id = rows
                    .runtime(&format!("blocked-{status}"), "offline", "8 days")
                    .await?;
                let issue_id = rows.issue(620 + offset as i32).await?;
                let task_id = rows.task(runtime_id, issue_id, status).await?;
                if status == "waiting_local_directory" {
                    sqlx::query("UPDATE agent_task_queue SET wait_reason = 'directory busy' WHERE id = $1")
                        .bind(task_id)
                        .execute(&rows.pool)
                        .await?;
                }
                let deleted = RuntimeTaskSweeper::new(
                    rows.pool.clone(),
                    Arc::new(TestLiveness {
                        available: false,
                        alive: HashSet::new(),
                        forgotten: Arc::new(Mutex::new(Vec::new())),
                        race_id: None,
                        pool: None,
                    }),
                    Arc::new(TaskService::new(rows.pool.clone(), Arc::new(Bus::new()))),
                    Arc::new(Bus::new()),
                    None,
                    DEFAULT_RECONNECT_GRACE,
                )
                .gc_runtime(runtime_id, Utc::now() - chrono::Duration::days(7))
                .await?;
                anyhow::ensure!(deleted.is_none(), "GC deleted runtime with {status} task");
                anyhow::ensure!(
                    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_task_queue WHERE id = $1 AND runtime_id = $2 AND completed_at IS NULL")
                        .bind(task_id)
                        .bind(runtime_id)
                        .fetch_one(&rows.pool)
                        .await?
                        == 1,
                    "GC changed protected {status} task"
                );
            }

            let bounded_runtime = rows
                .runtime("bounded-candidate", "offline", "100 years")
                .await?;
            let bounded_candidates = runtime::list_stale_offline_runtime_gc_candidates(
                &rows.pool,
                Utc::now() - chrono::Duration::days(7),
                1,
            )
            .await?;
            anyhow::ensure!(
                bounded_candidates == vec![Some(bounded_runtime)],
                "GC candidate query did not return the oldest known candidate at max_per_tick=1: {bounded_candidates:?}"
            );

            let active_runtime = rows.runtime("guard-active-agent", "offline", "8 days").await?;
            let active_agent = RecoveryRows::agent(
                &rows.pool,
                rows.workspace_id,
                rows.user_id,
                active_runtime,
                "guard-active-agent",
            )
            .await?;
            let sweeper = RuntimeTaskSweeper::new(
                rows.pool.clone(),
                Arc::new(TestLiveness {
                    available: false,
                    alive: HashSet::new(),
                    forgotten: Arc::new(Mutex::new(Vec::new())),
                    race_id: None,
                    pool: None,
                }),
                Arc::new(TaskService::new(rows.pool.clone(), Arc::new(Bus::new()))),
                Arc::new(Bus::new()),
                None,
                DEFAULT_RECONNECT_GRACE,
            );
            anyhow::ensure!(
                sweeper
                    .gc_runtime(active_runtime, Utc::now() - chrono::Duration::days(7))
                    .await?
                    .is_none(),
                "GC deleted runtime with bound agent"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent WHERE id = $1 AND runtime_id = $2")
                    .bind(active_agent)
                    .bind(active_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                    == 1,
                "bound agent was changed by blocked GC"
            );

            for (suffix, status, age) in [("fresh-guard", "offline", "1 day"), ("online-guard", "online", "8 days")] {
                let runtime_id = rows.runtime(suffix, status, age).await?;
                anyhow::ensure!(
                    sweeper
                        .gc_runtime(runtime_id, Utc::now() - chrono::Duration::days(7))
                        .await?
                        .is_none(),
                    "GC deleted {status} runtime that should be ineligible"
                );
            }
            let blocked_count = runtime::count_stale_offline_runtimes_blocked_by_tasks(
                &rows.pool,
                Utc::now() - chrono::Duration::days(7),
                GC_BLOCKED_LIMIT,
            )
            .await?
            .unwrap_or_default();
            anyhow::ensure!(
                blocked_count >= blocked_before + 5,
                "blocked runtime gauge grew from {blocked_before} to {blocked_count}, want all five fixture statuses"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("runtime GC guard contract failed");
        cleanup.expect("runtime GC guard fixture cleanup failed");
    }

    #[tokio::test]
    async fn production_runtime_gc_owner_lock_orders_enqueue_and_delete_safely() {
        let rows = GcRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for runtime GC owner lock contract",
        );
        let result = async {
            let writer_runtime = rows.runtime("writer-wins", "offline", "8 days").await?;
            let writer_issue = rows.issue(640).await?;
            let mut holder = rows.pool.begin().await?;
            sqlx::query("SELECT id FROM agent_runtime WHERE id = $1 FOR UPDATE")
                .bind(writer_runtime)
                .fetch_one(&mut *holder)
                .await?;
            let mut writer = rows.pool.acquire().await?;
            let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *writer)
                .await?;
            let agent_id = rows.helper_agent_id;
            let writer_handle = tokio::spawn(async move {
                sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority) \
                     SELECT $1, $2, $3, 'queued', 0 \
                     WHERE lock_task_owner_rows($1, $3, $2) RETURNING id",
                )
                .bind(agent_id)
                .bind(writer_runtime)
                .bind(writer_issue)
                .fetch_optional(&mut *writer)
                .await
            });
            wait_for_lock_wait(&rows.pool, writer_pid).await?;
            holder.commit().await?;
            let writer_task = writer_handle.await??.expect("owner-fenced writer inserted no task");
            let sweeper = RuntimeTaskSweeper::new(
                rows.pool.clone(),
                Arc::new(TestLiveness {
                    available: false,
                    alive: HashSet::new(),
                    forgotten: Arc::new(Mutex::new(Vec::new())),
                    race_id: None,
                    pool: None,
                }),
                Arc::new(TaskService::new(rows.pool.clone(), Arc::new(Bus::new()))),
                Arc::new(Bus::new()),
                None,
                DEFAULT_RECONNECT_GRACE,
            );
            anyhow::ensure!(
                sweeper
                    .gc_runtime(writer_runtime, Utc::now() - chrono::Duration::days(7))
                    .await?
                    .is_none(),
                "GC deleted runtime after owner-fenced enqueue committed"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_task_queue WHERE id = $1 AND runtime_id = $2 AND completed_at IS NULL")
                    .bind(writer_task)
                    .bind(writer_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                    == 1,
                "writer task was detached or deleted despite blocking GC"
            );

            let delete_runtime = rows.runtime("gc-wins-race", "offline", "8 days").await?;
            let terminal_issue = rows.issue(641).await?;
            let terminal_task = rows
                .task(delete_runtime, terminal_issue, "completed")
                .await?;
            mark_task_terminal(&rows.pool, terminal_task, "completed").await?;

            // Hold the terminal task row so GC stops after acquiring the runtime owner lock.
            // The enqueue must then wait behind GC; without gc_runtime's FOR UPDATE it would
            // commit here and this assertion would fail.
            let mut task_holder = rows.pool.begin().await?;
            sqlx::query("SELECT id FROM agent_task_queue WHERE id = $1 FOR UPDATE")
                .bind(terminal_task)
                .fetch_one(&mut *task_holder)
                .await?;
            let stale_before = Utc::now() - chrono::Duration::days(7);
            let gc_handle = tokio::spawn(async move {
                sweeper.gc_runtime(delete_runtime, stale_before).await
            });
            wait_for_blocked_task_detach(&rows.pool).await?;

            let delete_issue = rows.issue(642).await?;
            let mut writer = rows.pool.acquire().await?;
            let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *writer)
                .await?;
            let agent_id = rows.helper_agent_id;
            let writer_handle = tokio::spawn(async move {
                sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority) \
                     SELECT $1, $2, $3, 'queued', 0 \
                     WHERE lock_task_owner_rows($1, $3, $2) RETURNING id",
                )
                .bind(agent_id)
                .bind(delete_runtime)
                .bind(delete_issue)
                .fetch_optional(&mut *writer)
                .await
            });
            wait_for_lock_wait(&rows.pool, writer_pid).await?;
            task_holder.commit().await?;
            anyhow::ensure!(
                gc_handle.await??.is_some(),
                "GC did not win the owner-lock race"
            );
            anyhow::ensure!(
                writer_handle.await??.is_none(),
                "owner-fenced enqueue committed after GC had acquired the runtime owner lock"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_task_queue WHERE runtime_id = $1")
                    .bind(delete_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                    == 0,
                "deleted runtime retained an orphaned task"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("runtime GC owner-lock contract failed");
        cleanup.expect("runtime GC owner-lock fixture cleanup failed");
    }

    #[tokio::test]
    async fn production_runtime_gc_timeout_rolls_back_and_isolates_next_candidate() {
        let rows = GcRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for runtime GC timeout contract",
        );
        let result = async {
            let blocked_runtime = rows
                .runtime("timeout-blocked", "offline", "100 years")
                .await?;
            let blocked_chat: Uuid = sqlx::query_scalar(
                "INSERT INTO chat_session (workspace_id, agent_id, creator_id, runtime_id) \
                 VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(rows.workspace_id)
            .bind(rows.helper_agent_id)
            .bind(rows.user_id)
            .bind(blocked_runtime)
            .fetch_one(&rows.pool)
            .await?;
            let next_runtime = rows.runtime("timeout-next", "offline", "99 years").await?;

            // The runtime delete must apply chat_session's ON DELETE SET NULL. Holding that
            // row injects a delete-stage stall so the production operation budget cancels the
            // transaction; the next candidate must still run.
            let mut chat_holder = rows.pool.begin().await?;
            sqlx::query("SELECT id FROM chat_session WHERE id = $1 FOR UPDATE")
                .bind(blocked_chat)
                .fetch_one(&mut *chat_holder)
                .await?;
            let sweeper = RuntimeTaskSweeper::new(
                rows.pool.clone(),
                Arc::new(TestLiveness {
                    available: false,
                    alive: HashSet::new(),
                    forgotten: Arc::new(Mutex::new(Vec::new())),
                    race_id: None,
                    pool: None,
                }),
                Arc::new(TaskService::new(rows.pool.clone(), Arc::new(Bus::new()))),
                Arc::new(Bus::new()),
                None,
                DEFAULT_RECONNECT_GRACE,
            );
            let deleted = sweeper
                .gc_with_budget(Utc::now() - chrono::Duration::days(7))
                .await;
            chat_holder.commit().await?;

            anyhow::ensure!(
                deleted >= 1,
                "candidate timeout starved the following candidate"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runtime WHERE id = $1")
                    .bind(next_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                    == 0,
                "candidate after the timed-out candidate was not collected"
            );
            anyhow::ensure!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_runtime WHERE id = $1")
                    .bind(blocked_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                    == 1
                    && sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM chat_session WHERE id = $1 AND runtime_id = $2"
                    )
                    .bind(blocked_chat)
                    .bind(blocked_runtime)
                    .fetch_one(&rows.pool)
                    .await?
                        == 1,
                "timed-out delete did not roll back its runtime/chat transaction"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("runtime GC timeout/isolation contract failed");
        cleanup.expect("runtime GC timeout fixture cleanup failed");
    }
}
