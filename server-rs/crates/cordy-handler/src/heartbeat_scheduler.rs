use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cordy_db::models::AgentRuntime;
use cordy_db::queries::runtime;
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
        if agent_runtime.status == "online" && agent_runtime.last_seen_at.is_some() {
            if runtime::touch_agent_runtime_last_seen(&self.pool, agent_runtime.id).await? > 0 {
                return Ok(());
            }
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
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        HeartbeatSchedulerRuntime {
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
            Ok(Ok(())) => HeartbeatShutdownOutcome::Stopped,
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
