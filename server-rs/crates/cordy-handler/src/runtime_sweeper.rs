//! Bounded runtime liveness sweeper.
//!
//! Redis heartbeat presence protects a DB-stale runtime from being offlined.
//! Redis is only an optimisation: errors and timeouts deliberately fall back
//! to the race-safe DB stale predicate used before the liveness cache existed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::state::HandlerState;

const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const STALE_THRESHOLD: Duration = Duration::from_secs(150);
pub const DEFAULT_RECONNECT_GRACE: Duration = Duration::from_secs(3 * 60 * 60);
pub const MINIMUM_RECONNECT_GRACE: Duration = STALE_THRESHOLD;
const CANDIDATE_BATCH_SIZE: i32 = 500;
const OFFLINE_TASK_BATCH_SIZE: i32 = 500;
const RECONNECT_RETRY_BATCH_SIZE: i32 = 500;
const STALE_TASK_BATCH_SIZE: i32 = 500;
const QUEUED_TASK_BATCH_SIZE: i32 = 500;
const DELEGATED_RECOVERY_BATCH_SIZE: i32 = 100;
const CHAT_FINALIZE_BATCH_SIZE: i32 = 100;
const RUNTIME_GC_BATCH_SIZE: i32 = 100;
const RUNTIME_GC_BLOCKED_SCAN_LIMIT: i32 = 1_000;
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(300);
const RUNNING_TIMEOUT: Duration = Duration::from_secs(9_000);
const QUEUED_TTL: Duration = Duration::from_secs(2 * 60 * 60);
const CHAT_FINALIZE_GRACE: Duration = Duration::from_secs(60);
const OFFLINE_RUNTIME_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const RUNTIME_GC_TICK_TIMEOUT: Duration = Duration::from_secs(15);
const RUNTIME_GC_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);

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

pub struct RuntimeSweeper {
    state: HandlerState,
    clock: Arc<dyn Clock>,
    reconnect_grace: Duration,
}

impl RuntimeSweeper {
    pub fn from_state(state: HandlerState, reconnect_grace: Duration) -> Self {
        Self {
            state,
            clock: Arc::new(SystemClock),
            reconnect_grace: reconnect_grace.max(MINIMUM_RECONNECT_GRACE),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn start(self) -> RuntimeSweeperHandle {
        let cancel = CancellationToken::new();
        let child = cancel.child_token();
        let join = tokio::spawn(async move { self.run(child).await });
        RuntimeSweeperHandle {
            cancel,
            join: Some(join),
        }
    }

    async fn run(self, cancel: CancellationToken) {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Preserve Go's one-interval startup delay and avoid a rolling-deploy
        // thundering herd against the candidate query.
        interval.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = interval.tick() => {
                    if let Err(error) = self.run_once(&cancel).await {
                        tracing::warn!(%error, "runtime sweeper tick failed");
                    }
                }
            }
        }
    }

    /// Runs one bounded sweep. The cancellation token is a child of the
    /// server lifecycle root, so every database/cache stage is cancellable.
    pub async fn run_once(&self, cancel: &CancellationToken) -> anyhow::Result<SweepResult> {
        let now = self.clock.now();
        let (candidates, offlined) = isolate_stage(
            cancel,
            "stale runtimes",
            self.sweep_stale_runtimes(cancel, now),
        )
        .await?
        .unwrap_or_default();
        let failed_tasks = isolate_stage(
            cancel,
            "offline runtime tasks",
            self.sweep_offline_tasks(cancel, now),
        )
        .await?
        .unwrap_or_default();
        let expired_reconnect_retries = isolate_stage(
            cancel,
            "expired runtime reconnect retries",
            self.sweep_expired_reconnect_retries(cancel, now),
        )
        .await?
        .unwrap_or_default();
        let stale_tasks = isolate_stage(cancel, "stale tasks", self.sweep_stale_tasks(cancel, now))
            .await?
            .unwrap_or_default();
        let expired_queued_tasks = isolate_stage(
            cancel,
            "expired queued tasks",
            self.sweep_expired_queued_tasks(cancel, now),
        )
        .await?
        .unwrap_or_default();
        let delegated_recovery = isolate_stage(
            cancel,
            "delegated failure recovery",
            self.sweep_delegated_failure_recovery(cancel),
        )
        .await?
        .unwrap_or_default();
        let deferred_chat_finalizations = isolate_stage(
            cancel,
            "deferred chat finalizations",
            self.sweep_deferred_chat_finalizations(cancel, now),
        )
        .await?
        .unwrap_or_default();
        let runtime_gc = isolate_stage(cancel, "runtime GC", self.sweep_runtime_gc(cancel, now))
            .await?
            .unwrap_or_default();
        Ok(SweepResult {
            candidates,
            offlined,
            failed_tasks,
            expired_reconnect_retries,
            stale_tasks,
            expired_queued_tasks,
            delegated_recoveries_replayed: delegated_recovery.0,
            delegated_recoveries_exhausted: delegated_recovery.1,
            deferred_chat_finalizations,
            runtime_gc_blocked: runtime_gc.blocked,
            runtime_gc_deleted: runtime_gc.deleted,
            runtime_gc_failed: runtime_gc.failed,
        })
    }

    async fn sweep_stale_runtimes(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(usize, usize)> {
        let stale_before = now
            - chrono::Duration::from_std(STALE_THRESHOLD)
                .expect("runtime stale threshold fits chrono");
        let candidates = cancellable(
            cancel,
            cordy_db::queries::runtime::select_stale_online_runtimes(
                &self.state.pool,
                stale_before,
                CANDIDATE_BATCH_SIZE,
            ),
        )
        .await?;

        let candidate_ids = candidates
            .iter()
            .filter_map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        let to_offline = match self.filter_alive(cancel, &candidate_ids).await {
            Ok(ids) => ids,
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!(%error, "runtime sweeper liveness unavailable; using DB stale state");
                candidate_ids.clone()
            }
        };

        let rows = if to_offline.is_empty() {
            Vec::new()
        } else {
            cancellable(
                cancel,
                cordy_db::queries::runtime::mark_runtimes_offline_by_i_ds(
                    &self.state.pool,
                    to_offline,
                    stale_before,
                ),
            )
            .await?
        };

        let mut workspaces = HashSet::new();
        for row in &rows {
            let (Some(runtime_id), Some(workspace_id)) = (row.id, row.workspace_id) else {
                continue;
            };
            workspaces.insert(workspace_id);
            let event = cordy_analytics::runtime_offline(
                &row.owner_id.map(|id| id.to_string()).unwrap_or_default(),
                &workspace_id.to_string(),
                &runtime_id.to_string(),
                row.daemon_id.as_deref().unwrap_or_default(),
                &row.provider,
            );
            cordy_metrics::business_events::record_event(
                Some(self.state.analytics.as_ref()),
                self.state.business_metrics.as_deref(),
                &event,
            );
            if let Some(liveness) = self.state.runtime_liveness.as_ref() {
                if let Err(error) = liveness.forget(&runtime_id.to_string()).await {
                    tracing::warn!(%error, %runtime_id, "runtime sweeper liveness cleanup failed");
                }
            }
        }
        for workspace_id in &workspaces {
            self.state.bus.publish(&cordy_events::Event {
                event_type: cordy_protocol::EVENT_DAEMON_REGISTER.into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "system".into(),
                payload: serde_json::json!({ "action": "stale_sweep" }),
                ..Default::default()
            });
        }

        if !rows.is_empty() {
            tracing::info!(
                count = rows.len(),
                workspaces = workspaces.len(),
                "runtime sweeper marked stale runtimes offline"
            );
        }

        Ok((candidates.len(), rows.len()))
    }

    async fn sweep_offline_tasks(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<usize> {
        let task_stale_before = now
            - chrono::Duration::from_std(self.reconnect_grace)
                .expect("runtime reconnect grace fits chrono");
        let failed_tasks = cancellable(
            cancel,
            cordy_db::queries::runtime::fail_tasks_for_offline_runtimes(
                &self.state.pool,
                task_stale_before,
                OFFLINE_TASK_BATCH_SIZE,
            ),
        )
        .await?;
        if !failed_tasks.is_empty() {
            tracing::info!(
                count = failed_tasks.len(),
                "runtime sweeper failed tasks beyond reconnect grace"
            );
            // The database transition has committed. Finish its idempotent
            // side effects even if root cancellation arrives; owned shutdown
            // still bounds the enclosing worker and aborts after its grace.
            self.state.tasks.handle_failed_tasks(&failed_tasks).await;
        }
        Ok(failed_tasks.len())
    }

    async fn sweep_expired_reconnect_retries(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<usize> {
        let retry_before = now
            - chrono::Duration::from_std(self.reconnect_grace)
                .expect("runtime reconnect grace fits chrono");
        let runtime_fresh_after = now
            - chrono::Duration::from_std(STALE_THRESHOLD)
                .expect("runtime stale threshold fits chrono");
        let failed = cancellable(
            cancel,
            cordy_db::queries::agent::fail_expired_runtime_reconnect_retries(
                &self.state.pool,
                retry_before,
                runtime_fresh_after,
                RECONNECT_RETRY_BATCH_SIZE,
            ),
        )
        .await?;
        if !failed.is_empty() {
            tracing::info!(
                count = failed.len(),
                "runtime sweeper expired reconnect retries"
            );
        }
        self.handle_failed(&failed).await;
        Ok(failed.len())
    }

    async fn sweep_stale_tasks(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<usize> {
        let failed = cancellable(
            cancel,
            cordy_db::queries::agent::fail_stale_tasks(
                &self.state.pool,
                now - chrono::Duration::from_std(DISPATCH_TIMEOUT)
                    .expect("dispatch timeout fits chrono"),
                now,
                now - chrono::Duration::from_std(STALE_THRESHOLD)
                    .expect("runtime stale threshold fits chrono"),
                now - chrono::Duration::from_std(self.reconnect_grace)
                    .expect("runtime reconnect grace fits chrono"),
                now - chrono::Duration::from_std(RUNNING_TIMEOUT)
                    .expect("running timeout fits chrono"),
                STALE_TASK_BATCH_SIZE,
            ),
        )
        .await?;
        if !failed.is_empty() {
            tracing::info!(count = failed.len(), "runtime sweeper failed stale tasks");
            self.state.tasks.capture_lease_expired_tasks(&failed).await;
        }
        self.handle_failed(&failed).await;
        Ok(failed.len())
    }

    async fn sweep_expired_queued_tasks(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<usize> {
        let failed = cancellable(
            cancel,
            cordy_db::queries::agent::expire_stale_queued_tasks(
                &self.state.pool,
                now - chrono::Duration::from_std(QUEUED_TTL).expect("queued TTL fits chrono"),
                QUEUED_TASK_BATCH_SIZE,
            ),
        )
        .await?;
        if !failed.is_empty() {
            tracing::info!(count = failed.len(), "runtime sweeper expired queued tasks");
            self.state.tasks.capture_queued_expired_tasks(&failed).await;
        }
        self.handle_failed(&failed).await;
        Ok(failed.len())
    }

    async fn handle_failed(&self, failed: &[cordy_db::models::AgentTaskQueue]) {
        if failed.is_empty() {
            return;
        }
        self.state.tasks.handle_failed_tasks(failed).await;
    }

    async fn sweep_delegated_failure_recovery(
        &self,
        cancel: &CancellationToken,
    ) -> anyhow::Result<(i32, i32)> {
        let result = cancellable(cancel, async {
            self.state
                .tasks
                .recover_pending_delegated_failures(DELEGATED_RECOVERY_BATCH_SIZE)
                .await
                .map_err(anyhow::Error::from)
        })
        .await?;
        if result.replayed > 0 {
            tracing::info!(
                count = result.replayed,
                "runtime sweeper replayed delegated failure recoveries"
            );
        }
        if result.exhausted > 0 {
            tracing::warn!(
                count = result.exhausted,
                "runtime sweeper exhausted delegated failure recoveries"
            );
        }
        Ok((result.replayed, result.exhausted))
    }

    async fn sweep_deferred_chat_finalizations(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<usize> {
        let deferred_before = now
            - chrono::Duration::from_std(CHAT_FINALIZE_GRACE)
                .expect("chat finalize grace fits chrono");
        let tasks = cancellable(
            cancel,
            cordy_db::queries::agent::list_chat_finalize_deferred_expired(
                &self.state.pool,
                deferred_before,
                CHAT_FINALIZE_BATCH_SIZE,
            ),
        )
        .await?;
        for task in &tasks {
            cancellable(cancel, async {
                self.state
                    .tasks
                    .finalize_deferred_cancelled_chat(task.id)
                    .await;
                Ok(())
            })
            .await?;
        }
        if !tasks.is_empty() {
            tracing::info!(
                count = tasks.len(),
                "runtime sweeper settled deferred chat cancellations"
            );
        }
        Ok(tasks.len())
    }

    async fn sweep_runtime_gc(
        &self,
        cancel: &CancellationToken,
        now: DateTime<Utc>,
    ) -> anyhow::Result<RuntimeGcResult> {
        let stale_before = now
            - chrono::Duration::from_std(OFFLINE_RUNTIME_TTL)
                .expect("offline runtime TTL fits chrono");
        let deadline = tokio::time::Instant::now() + RUNTIME_GC_TICK_TIMEOUT;
        let mut result = RuntimeGcResult::default();

        match cancellable_with_timeout(
            cancel,
            remaining_operation_budget(deadline)?,
            cordy_db::queries::runtime::count_stale_offline_runtimes_blocked_by_tasks(
                &self.state.pool,
                stale_before,
                RUNTIME_GC_BLOCKED_SCAN_LIMIT,
            ),
        )
        .await
        {
            Ok(blocked) => {
                result.blocked = blocked.unwrap_or_default();
                if let Some(metrics) = self.state.business_metrics.as_deref() {
                    metrics.set_runtime_gc_blocked(result.blocked);
                }
                if result.blocked > 0 {
                    tracing::debug!(
                        count = result.blocked,
                        count_capped = result.blocked == i64::from(RUNTIME_GC_BLOCKED_SCAN_LIMIT),
                        "runtime GC found stale runtimes blocked by tasks"
                    );
                }
            }
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => {
                tracing::warn!(%error, "runtime GC blocked observation failed");
                if let Some(metrics) = self.state.business_metrics.as_deref() {
                    metrics.record_runtime_gc_blocked_observation_failed();
                }
            }
        }

        let candidates = match cancellable_with_timeout(
            cancel,
            remaining_operation_budget(deadline)?,
            cordy_db::queries::runtime::list_stale_offline_runtime_gc_candidates(
                &self.state.pool,
                stale_before,
                RUNTIME_GC_BATCH_SIZE,
            ),
        )
        .await
        {
            Ok(candidates) => candidates.into_iter().flatten().collect::<Vec<_>>(),
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => {
                if let Some(metrics) = self.state.business_metrics.as_deref() {
                    metrics.record_runtime_gc_failed();
                }
                return Err(error);
            }
        };

        let mut workspaces = HashSet::new();
        for (index, runtime_id) in candidates.iter().copied().enumerate() {
            let Ok(budget) = remaining_operation_budget(deadline) else {
                tracing::info!(
                    deleted = result.deleted,
                    remaining_candidates = candidates.len() - index,
                    "runtime GC tick budget exhausted"
                );
                break;
            };
            match self
                .gc_runtime(cancel, runtime_id, stale_before, budget)
                .await
            {
                Ok(RuntimeGcAttempt::Deleted(workspace_id)) => {
                    result.deleted += 1;
                    workspaces.insert(workspace_id);
                    if let Some(metrics) = self.state.business_metrics.as_deref() {
                        metrics.record_runtime_gc_deleted();
                    }
                }
                Ok(RuntimeGcAttempt::TaskBlocked) => {
                    tracing::warn!(
                        %runtime_id,
                        "runtime GC candidate gained a non-terminal task"
                    );
                }
                Ok(RuntimeGcAttempt::Ineligible) => {}
                Err(error) if cancel.is_cancelled() => return Err(error),
                Err(error) => {
                    if tokio::time::Instant::now() >= deadline {
                        tracing::info!(
                            deleted = result.deleted,
                            remaining_candidates = candidates.len() - index,
                            "runtime GC tick budget exhausted"
                        );
                        break;
                    }
                    result.failed += 1;
                    tracing::warn!(%error, %runtime_id, "runtime GC failed to delete candidate");
                    if let Some(metrics) = self.state.business_metrics.as_deref() {
                        metrics.record_runtime_gc_failed();
                    }
                }
            }
        }

        if result.deleted > 0 {
            tracing::info!(
                count = result.deleted,
                workspaces = workspaces.len(),
                "runtime GC deleted stale offline runtimes"
            );
            for workspace_id in workspaces {
                self.state.bus.publish(&cordy_events::Event {
                    event_type: cordy_protocol::EVENT_DAEMON_REGISTER.into(),
                    workspace_id: workspace_id.to_string(),
                    actor_type: "system".into(),
                    payload: serde_json::json!({ "action": "runtime_gc" }),
                    ..Default::default()
                });
            }
        }
        Ok(result)
    }

    async fn gc_runtime(
        &self,
        cancel: &CancellationToken,
        runtime_id: Uuid,
        stale_before: DateTime<Utc>,
        timeout: Duration,
    ) -> anyhow::Result<RuntimeGcAttempt> {
        cancellable_with_timeout(cancel, timeout, async {
            let mut tx = self.state.pool.begin().await?;
            let Some(runtime) =
                cordy_db::queries::runtime::lock_agent_runtime(&mut *tx, runtime_id).await?
            else {
                return Ok(RuntimeGcAttempt::Ineligible);
            };
            let eligible = cordy_db::queries::runtime::is_agent_runtime_eligible_for_gc(
                &mut *tx,
                runtime_id,
                stale_before,
            )
            .await?
            .unwrap_or(false);
            if !eligible {
                return Ok(RuntimeGcAttempt::Ineligible);
            }
            let undrained = cordy_db::queries::runtime::count_undrained_tasks_by_runtime_or_agent(
                &mut *tx,
                vec![runtime_id],
                Vec::new(),
            )
            .await?
            .unwrap_or_default();
            if undrained > 0 {
                return Ok(RuntimeGcAttempt::TaskBlocked);
            }
            cordy_db::queries::runtime::unbind_tasks_from_runtime(&mut *tx, runtime_id).await?;
            let remaining =
                cordy_db::queries::runtime::count_tasks_by_runtime(&mut *tx, runtime_id)
                    .await?
                    .unwrap_or_default();
            anyhow::ensure!(
                remaining == 0,
                "task history still references runtime after detach"
            );
            let deleted =
                cordy_db::queries::runtime::delete_agent_runtime(&mut *tx, runtime_id).await?;
            anyhow::ensure!(deleted == 1, "runtime disappeared while locked");
            tx.commit().await?;
            Ok(RuntimeGcAttempt::Deleted(runtime.workspace_id))
        })
        .await
    }

    async fn filter_alive(
        &self,
        cancel: &CancellationToken,
        candidates: &[Uuid],
    ) -> anyhow::Result<Vec<Uuid>> {
        let Some(liveness) = self.state.runtime_liveness.as_ref() else {
            return Ok(candidates.to_vec());
        };
        let ids = candidates.iter().map(Uuid::to_string).collect::<Vec<_>>();
        let alive = cancellable(cancel, liveness.is_alive_batch(&ids)).await?;
        Ok(filter_candidate_ids(candidates, &alive))
    }
}

fn filter_candidate_ids(candidates: &[Uuid], alive: &HashMap<String, bool>) -> Vec<Uuid> {
    candidates
        .iter()
        .filter(|id| !alive.get(&id.to_string()).copied().unwrap_or(false))
        .copied()
        .collect()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepResult {
    pub candidates: usize,
    pub offlined: usize,
    pub failed_tasks: usize,
    pub expired_reconnect_retries: usize,
    pub stale_tasks: usize,
    pub expired_queued_tasks: usize,
    pub delegated_recoveries_replayed: i32,
    pub delegated_recoveries_exhausted: i32,
    pub deferred_chat_finalizations: usize,
    pub runtime_gc_blocked: i64,
    pub runtime_gc_deleted: usize,
    pub runtime_gc_failed: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RuntimeGcResult {
    blocked: i64,
    deleted: usize,
    failed: usize,
}

enum RuntimeGcAttempt {
    Deleted(Uuid),
    TaskBlocked,
    Ineligible,
}

fn remaining_operation_budget(deadline: tokio::time::Instant) -> anyhow::Result<Duration> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    anyhow::ensure!(!remaining.is_zero(), "runtime GC tick budget exhausted");
    Ok(remaining.min(RUNTIME_GC_OPERATION_TIMEOUT))
}

async fn isolate_stage<T>(
    cancel: &CancellationToken,
    name: &'static str,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<Option<T>> {
    match future.await {
        Ok(value) => Ok(Some(value)),
        Err(error) if cancel.is_cancelled() => Err(error),
        Err(error) => {
            tracing::warn!(%error, stage = name, "runtime sweeper stage failed");
            Ok(None)
        }
    }
}

async fn cancellable<T>(
    cancel: &CancellationToken,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("runtime sweeper cancelled")),
        result = future => result,
    }
}

async fn cancellable_with_timeout<T>(
    cancel: &CancellationToken,
    timeout: Duration,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::select! {
        _ = cancel.cancelled() => Err(anyhow::anyhow!("runtime sweeper cancelled")),
        result = tokio::time::timeout(timeout, future) => {
            result.map_err(|_| anyhow::anyhow!("runtime sweeper operation timed out"))?
        }
    }
}

pub struct RuntimeSweeperCancellation(CancellationToken);

impl RuntimeSweeperCancellation {
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

pub struct RuntimeSweeperHandle {
    cancel: CancellationToken,
    join: Option<JoinHandle<()>>,
}

impl RuntimeSweeperHandle {
    pub fn cancellation(&self) -> RuntimeSweeperCancellation {
        RuntimeSweeperCancellation(self.cancel.clone())
    }

    pub async fn shutdown(mut self, timeout: Duration) {
        self.cancel.cancel();
        let Some(mut join) = self.join.take() else {
            return;
        };
        if tokio::time::timeout(timeout, &mut join).await.is_err() {
            tracing::warn!(
                ?timeout,
                "runtime sweeper shutdown timed out; aborting task"
            );
            join.abort();
        }
    }
}

impl Drop for RuntimeSweeperHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[test]
    fn bounds_and_timing_match_production_contract() {
        assert_eq!(SWEEP_INTERVAL, Duration::from_secs(30));
        assert_eq!(STALE_THRESHOLD, Duration::from_secs(150));
        assert_eq!(DEFAULT_RECONNECT_GRACE, Duration::from_secs(10_800));
        assert_eq!(CANDIDATE_BATCH_SIZE, 500);
        assert_eq!(OFFLINE_TASK_BATCH_SIZE, 500);
        assert_eq!(RECONNECT_RETRY_BATCH_SIZE, 500);
        assert_eq!(STALE_TASK_BATCH_SIZE, 500);
        assert_eq!(QUEUED_TASK_BATCH_SIZE, 500);
        assert_eq!(DELEGATED_RECOVERY_BATCH_SIZE, 100);
        assert_eq!(CHAT_FINALIZE_BATCH_SIZE, 100);
        assert_eq!(RUNTIME_GC_BATCH_SIZE, 100);
        assert_eq!(RUNTIME_GC_BLOCKED_SCAN_LIMIT, 1_000);
        assert_eq!(DISPATCH_TIMEOUT, Duration::from_secs(300));
        assert_eq!(RUNNING_TIMEOUT, Duration::from_secs(9_000));
        assert_eq!(QUEUED_TTL, Duration::from_secs(7_200));
        assert_eq!(CHAT_FINALIZE_GRACE, Duration::from_secs(60));
        assert_eq!(OFFLINE_RUNTIME_TTL, Duration::from_secs(604_800));
        assert_eq!(RUNTIME_GC_TICK_TIMEOUT, Duration::from_secs(15));
        assert_eq!(RUNTIME_GC_OPERATION_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn reconnect_grace_is_clamped_above_heartbeat_freshness() {
        assert_eq!(Duration::from_secs(1).max(STALE_THRESHOLD), STALE_THRESHOLD);
    }

    #[test]
    fn injected_clock_is_stable() {
        let now = DateTime::parse_from_rfc3339("2026-08-23T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock: Arc<dyn Clock> = Arc::new(FixedClock(now));
        assert_eq!(clock.now(), now);
    }

    #[test]
    fn redis_presence_only_protects_known_alive_candidates() {
        let alive_id = Uuid::from_u128(1);
        let absent_id = Uuid::from_u128(2);
        let missing_result_id = Uuid::from_u128(3);
        let alive = HashMap::from([(alive_id.to_string(), true), (absent_id.to_string(), false)]);

        assert_eq!(
            filter_candidate_ids(&[alive_id, absent_id, missing_result_id], &alive),
            vec![absent_id, missing_result_id]
        );
    }
}
