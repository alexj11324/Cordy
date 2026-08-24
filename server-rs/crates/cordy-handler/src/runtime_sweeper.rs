use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use cordy_db::queries::{agent, runtime};
use cordy_events::{Bus, Event};
use cordy_service::task_service::TaskService;
use serde_json::json;
use sqlx::PgPool;

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

#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeTaskSweepReport {
    pub runtimes_offline: usize,
    pub tasks_failed: usize,
    pub queued_expired: usize,
    pub recoveries_replayed: usize,
    pub recoveries_exhausted: usize,
    pub chats_finalized: usize,
}

pub struct RuntimeTaskSweeper {
    pool: PgPool,
    liveness: Arc<dyn LivenessStore>,
    tasks: Arc<TaskService>,
    bus: Arc<Bus>,
    reconnect_grace: Duration,
}

impl RuntimeTaskSweeper {
    pub fn new(
        pool: PgPool,
        liveness: Arc<dyn LivenessStore>,
        tasks: Arc<TaskService>,
        bus: Arc<Bus>,
        reconnect_grace: Duration,
    ) -> Self {
        Self {
            pool,
            liveness,
            tasks,
            bus,
            reconnect_grace: reconnect_grace.max(Duration::from_secs(150)),
        }
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
