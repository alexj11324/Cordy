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
}
