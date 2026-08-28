use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use patchbay_db::models::AgentRuntime;
use patchbay_db::queries::runtime;
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const DEFAULT_BATCH_INTERVAL: Duration = Duration::from_secs(30);
const FINAL_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);

#[async_trait]
pub trait HeartbeatScheduler: Send + Sync {
    async fn schedule(&self, runtime: &AgentRuntime) -> anyhow::Result<()>;
}

pub struct PassthroughHeartbeatScheduler {
    pool: PgPool,
}

impl PassthroughHeartbeatScheduler {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HeartbeatScheduler for PassthroughHeartbeatScheduler {
    async fn schedule(&self, agent_runtime: &AgentRuntime) -> anyhow::Result<()> {
        if agent_runtime.status == "online"
            && agent_runtime.last_seen_at.is_some()
            && runtime::touch_agent_runtime_last_seen(&self.pool, agent_runtime.id).await? > 0
        {
            return Ok(());
        }
        runtime::mark_agent_runtime_online(&self.pool, agent_runtime.id).await?;
        Ok(())
    }
}

pub struct BatchedHeartbeatScheduler {
    pool: PgPool,
    fallback: PassthroughHeartbeatScheduler,
    interval: Duration,
    pending: Mutex<HashSet<Uuid>>,
}

impl BatchedHeartbeatScheduler {
    pub fn new(pool: PgPool, interval: Duration) -> Self {
        let interval = if interval.is_zero() {
            DEFAULT_BATCH_INTERVAL
        } else {
            interval
        };
        Self {
            fallback: PassthroughHeartbeatScheduler::new(pool.clone()),
            pool,
            interval,
            pending: Mutex::new(HashSet::new()),
        }
    }

    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> HeartbeatSchedulerRuntime {
        let scheduler = self.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        HeartbeatSchedulerRuntime {
            scheduler,
            cancel,
            task: Some(task),
        }
    }

    pub async fn flush_once(&self) {
        let ids = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending.drain().collect::<Vec<_>>()
        };
        if ids.is_empty() {
            return;
        }
        match runtime::touch_agent_runtimes_last_seen_batch(&self.pool, ids.clone()).await {
            Ok(rows) if rows < ids.len() as u64 => tracing::info!(
                scheduled = ids.len(),
                affected = rows,
                "heartbeat batch flush: some runtimes raced to offline"
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!(
                scheduled = ids.len(),
                %error,
                "heartbeat batch flush failed"
            ),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    async fn run(&self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tokio::time::timeout(FINAL_FLUSH_TIMEOUT, self.flush_once()).await;
                    return;
                }
                _ = ticker.tick() => self.flush_once().await,
            }
        }
    }
}

#[async_trait]
impl HeartbeatScheduler for BatchedHeartbeatScheduler {
    async fn schedule(&self, agent_runtime: &AgentRuntime) -> anyhow::Result<()> {
        if agent_runtime.status != "online" || agent_runtime.last_seen_at.is_none() {
            return self.fallback.schedule(agent_runtime).await;
        }
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(agent_runtime.id);
        Ok(())
    }
}

pub struct HeartbeatSchedulerRuntime {
    scheduler: Arc<BatchedHeartbeatScheduler>,
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl HeartbeatSchedulerRuntime {
    pub async fn shutdown(mut self) -> HeartbeatShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return HeartbeatShutdownOutcome::Panicked;
        };
        match tokio::time::timeout(FINAL_FLUSH_TIMEOUT, &mut task).await {
            Ok(Ok(())) => {
                // Go's Stop performs a second bounded drain after Run exits.
                // This catches an ID scheduled after parent cancellation won
                // Run's select but before all request producers stopped.
                match tokio::time::timeout(FINAL_FLUSH_TIMEOUT, self.scheduler.flush_once()).await {
                    Ok(()) => HeartbeatShutdownOutcome::Stopped,
                    Err(_) => HeartbeatShutdownOutcome::TimedOut,
                }
            }
            Ok(Err(_)) => HeartbeatShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                HeartbeatShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for HeartbeatSchedulerRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use patchbay_db::dbid::new_v7;

    struct RuntimeRows {
        pool: PgPool,
        workspace_id: Uuid,
    }

    impl RuntimeRows {
        async fn required() -> Self {
            let url = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL is required for heartbeat worker contracts");
            let pool = PgPool::connect(&url)
                .await
                .expect("heartbeat contract requires a reachable migrated PostgreSQL");
            let workspace_id = new_v7();
            sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, $2, $3)")
                .bind(workspace_id)
                .bind("Rust heartbeat contract")
                .bind(format!("rust-heartbeat-{workspace_id}"))
                .execute(&pool)
                .await
                .expect("insert heartbeat contract workspace");
            Self { pool, workspace_id }
        }

        async fn runtime(&self, suffix: &str, status: &str, seen: bool) -> AgentRuntime {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO agent_runtime \
                 (id, workspace_id, daemon_id, name, runtime_mode, provider, status, last_seen_at) \
                 VALUES ($1, $2, $3, $4, 'local', $5, $6, \
                    CASE WHEN $7 THEN now() - interval '1 day' ELSE NULL END)",
            )
            .bind(id)
            .bind(self.workspace_id)
            .bind(format!("heartbeat-{suffix}"))
            .bind(format!("Heartbeat {suffix}"))
            .bind(format!("provider-{suffix}"))
            .bind(status)
            .bind(seen)
            .execute(&self.pool)
            .await
            .expect("insert heartbeat contract runtime");
            self.get(id).await
        }

        async fn get(&self, id: Uuid) -> AgentRuntime {
            runtime::get_agent_runtime(&self.pool, id)
                .await
                .expect("read heartbeat contract runtime")
                .expect("heartbeat contract runtime exists")
        }

        async fn cleanup(&self) {
            sqlx::query("DELETE FROM workspace WHERE id = $1")
                .bind(self.workspace_id)
                .execute(&self.pool)
                .await
                .expect("clean heartbeat contract workspace");
        }
    }

    impl Drop for RuntimeRows {
        fn drop(&mut self) {
            let pool = self.pool.clone();
            let workspace_id = self.workspace_id;
            // Do not leave fixture deletion to a detached task that can be
            // cancelled when the test runtime tears down.
            let _ = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build heartbeat cleanup executor");
                runtime.block_on(async move {
                    let _ = sqlx::query("DELETE FROM workspace WHERE id = $1")
                        .bind(workspace_id)
                        .execute(&pool)
                        .await;
                });
            })
            .join();
        }
    }

    fn seen(runtime: &AgentRuntime) -> DateTime<Utc> {
        runtime
            .last_seen_at
            .unwrap_or_else(|| panic!("runtime {} must have last_seen_at", runtime.id))
    }

    async fn wait_for_pending(scheduler: &BatchedHeartbeatScheduler, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if scheduler.pending_count() == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("heartbeat pending-count deadline");
    }

    #[tokio::test]
    async fn production_heartbeat_coalesces_batches_and_recovers_status_races() {
        let rows = RuntimeRows::required().await;
        let first = rows.runtime("first", "online", true).await;
        let second = rows.runtime("second", "online", true).await;
        let offline = rows.runtime("offline", "offline", false).await;
        let passthrough_race = rows.runtime("passthrough-race", "online", true).await;
        let batch_race = rows.runtime("batch-race", "online", true).await;
        let scheduler = BatchedHeartbeatScheduler::new(rows.pool.clone(), Duration::from_secs(60));

        scheduler.schedule(&first).await.expect("schedule first");
        scheduler
            .schedule(&first)
            .await
            .expect("coalesce duplicate first");
        scheduler.schedule(&second).await.expect("schedule second");
        assert_eq!(scheduler.pending_count(), 2);
        assert_eq!(seen(&rows.get(first.id).await), seen(&first));
        assert_eq!(seen(&rows.get(second.id).await), seen(&second));

        scheduler.flush_once().await;
        assert_eq!(scheduler.pending_count(), 0);
        assert!(seen(&rows.get(first.id).await) > seen(&first));
        assert!(seen(&rows.get(second.id).await) > seen(&second));

        scheduler
            .schedule(&offline)
            .await
            .expect("offline heartbeat uses synchronous fallback");
        let recovered_offline = rows.get(offline.id).await;
        assert_eq!(recovered_offline.status, "online");
        assert!(recovered_offline.last_seen_at.is_some());
        assert_eq!(scheduler.pending_count(), 0);

        let never_seen = rows.runtime("never-seen", "online", false).await;
        scheduler
            .schedule(&never_seen)
            .await
            .expect("never-seen heartbeat uses online fallback");
        let never_seen_after = rows.get(never_seen.id).await;
        assert_eq!(never_seen_after.status, "online");
        assert!(never_seen_after.last_seen_at.is_some());

        let passthrough_seen = rows.runtime("passthrough-seen", "online", true).await;
        let passthrough_seen_at = seen(&passthrough_seen);
        PassthroughHeartbeatScheduler::new(rows.pool.clone())
            .schedule(&passthrough_seen)
            .await
            .expect("already-seen online heartbeat touches in place");
        assert!(seen(&rows.get(passthrough_seen.id).await) > passthrough_seen_at);

        sqlx::query("UPDATE agent_runtime SET status = 'offline' WHERE id = $1")
            .bind(passthrough_race.id)
            .execute(&rows.pool)
            .await
            .expect("race passthrough runtime offline");
        PassthroughHeartbeatScheduler::new(rows.pool.clone())
            .schedule(&passthrough_race)
            .await
            .expect("touch miss falls through to mark online");
        assert_eq!(rows.get(passthrough_race.id).await.status, "online");

        scheduler
            .schedule(&batch_race)
            .await
            .expect("schedule online batch-race snapshot");
        sqlx::query("UPDATE agent_runtime SET status = 'offline' WHERE id = $1")
            .bind(batch_race.id)
            .execute(&rows.pool)
            .await
            .expect("race batched runtime offline");
        scheduler.flush_once().await;
        let still_offline = rows.get(batch_race.id).await;
        assert_eq!(still_offline.status, "offline");
        assert_eq!(seen(&still_offline), seen(&batch_race));
        scheduler
            .schedule(&still_offline)
            .await
            .expect("next heartbeat self-heals offline row");
        assert_eq!(rows.get(batch_race.id).await.status, "online");
        assert_eq!(scheduler.pending_count(), 0);
        rows.cleanup().await;
    }

    #[tokio::test]
    async fn production_heartbeat_shutdown_flushes_pending_and_late_schedule() {
        let rows = RuntimeRows::required().await;
        let pending = rows.runtime("shutdown-pending", "online", true).await;
        let late = rows.runtime("shutdown-late", "online", true).await;
        let pending_seen = seen(&pending);
        let late_seen = seen(&late);
        let scheduler = Arc::new(BatchedHeartbeatScheduler::new(
            rows.pool.clone(),
            Duration::from_millis(20),
        ));
        let runtime = scheduler.clone().start(CancellationToken::new());
        scheduler
            .schedule(&pending)
            .await
            .expect("schedule before shutdown");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if seen(&rows.get(pending.id).await) > pending_seen {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("heartbeat ticker flush deadline");

        let mut lock = rows.pool.begin().await.expect("begin runtime row lock");
        sqlx::query("SELECT id FROM agent_runtime WHERE id = $1 FOR UPDATE")
            .bind(pending.id)
            .fetch_one(&mut *lock)
            .await
            .expect("lock pending runtime row");
        let shutdown = tokio::spawn(async move { runtime.shutdown().await });
        wait_for_pending(&scheduler, 0).await;
        scheduler
            .schedule(&late)
            .await
            .expect("schedule after cancellation won first drain");
        assert_eq!(scheduler.pending_count(), 1);
        lock.commit().await.expect("release pending runtime row");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), shutdown)
                .await
                .expect("heartbeat shutdown deadline")
                .expect("heartbeat shutdown join"),
            HeartbeatShutdownOutcome::Stopped
        );
        assert_eq!(scheduler.pending_count(), 0);
        assert!(seen(&rows.get(pending.id).await) > pending_seen);
        assert!(seen(&rows.get(late.id).await) > late_seen);
        rows.cleanup().await;
    }
}
