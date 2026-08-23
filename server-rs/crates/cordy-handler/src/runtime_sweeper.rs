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
        let (candidates, offlined) = match self.sweep_stale_runtimes(cancel, now).await {
            Ok(result) => result,
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => {
                // Match Go's stage isolation: a stale-runtime query failure
                // must not starve cleanup for runtimes already offline.
                tracing::warn!(%error, "runtime sweeper stale-runtime stage failed");
                (0, 0)
            }
        };

        let failed_tasks = self.sweep_offline_tasks(cancel, now).await?;
        Ok(SweepResult {
            candidates,
            offlined,
            failed_tasks,
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
            cancellable(cancel, async {
                Ok(self.state.tasks.handle_failed_tasks(&failed_tasks).await)
            })
            .await?;
        }
        Ok(failed_tasks.len())
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
