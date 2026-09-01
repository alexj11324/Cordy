//! Reconciles managed IM installation capacity after subscription changes.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{stream, StreamExt};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::automation_failure_monitor::ShutdownOutcome;
use crate::task_service::{HostedCapacityPolicy, TaskService};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const RECONCILE_CONCURRENCY: usize = 8;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn reconciliation_limit(policy: HostedCapacityPolicy) -> Option<Option<i64>> {
    match policy {
        HostedCapacityPolicy::Bypass | HostedCapacityPolicy::Unlimited => Some(None),
        HostedCapacityPolicy::Limited(limit) => Some(Some(limit)),
        HostedCapacityPolicy::Disabled | HostedCapacityPolicy::Unavailable => None,
    }
}

#[derive(Clone)]
pub struct HostedInstallationReconciler {
    pool: sqlx::PgPool,
    tasks: Arc<TaskService>,
    enabled: bool,
}

impl HostedInstallationReconciler {
    pub fn new(pool: sqlx::PgPool, tasks: Arc<TaskService>, enabled: bool) -> Self {
        Self {
            pool,
            tasks,
            enabled,
        }
    }

    pub fn start(self, cancel: CancellationToken) -> HostedInstallationReconcilerRuntime {
        if !self.enabled {
            return HostedInstallationReconcilerRuntime::disabled();
        }
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        HostedInstallationReconcilerRuntime {
            cancel,
            task: Some(task),
        }
    }

    async fn run(self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if cancel.is_cancelled() {
                return;
            }
            if let Err(error) = self.run_once(&cancel).await {
                tracing::warn!(%error, "hosted installation capacity sweep failed");
            }
        }
    }

    pub async fn run_once(&self, cancel: &CancellationToken) -> anyhow::Result<()> {
        let workspace_ids =
            patchbay_db::queries::channel::list_hosted_installation_workspaces(&self.pool).await?;
        stream::iter(workspace_ids)
            .for_each_concurrent(RECONCILE_CONCURRENCY, |workspace_id| async move {
                let policy = tokio::select! {
                    _ = cancel.cancelled() => return,
                    value = self.tasks.hosted_im_installation_capacity(workspace_id) => value,
                };
                let Some(limit) = reconciliation_limit(policy) else {
                    // An unavailable policy preserves the last authoritative
                    // pause state. Self-hosted mode never starts this worker.
                    return;
                };
                match patchbay_db::queries::channel::reconcile_hosted_installation_capacity(
                    &self.pool,
                    workspace_id,
                    limit,
                )
                .await
                {
                    Ok(result) if !result.paused.is_empty() || !result.resumed.is_empty() => {
                        tracing::info!(
                            %workspace_id,
                            paused = result.paused.len(),
                            resumed = result.resumed.len(),
                            "reconciled hosted installation capacity"
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, %workspace_id, "failed to reconcile hosted installation capacity");
                    }
                }
            })
            .await;
        Ok(())
    }
}

pub struct HostedInstallationReconcilerRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl HostedInstallationReconcilerRuntime {
    fn disabled() -> Self {
        Self {
            cancel: CancellationToken::new(),
            task: None,
        }
    }

    pub async fn shutdown(mut self, timeout: Duration) -> ShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return ShutdownOutcome::Disabled;
        };
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => ShutdownOutcome::Stopped,
            Ok(Err(_)) => ShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                ShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for HostedInstallationReconcilerRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_authoritative_capacity_changes_pause_state() {
        assert_eq!(
            reconciliation_limit(HostedCapacityPolicy::Unavailable),
            None
        );
        assert_eq!(reconciliation_limit(HostedCapacityPolicy::Disabled), None);
        assert_eq!(
            reconciliation_limit(HostedCapacityPolicy::Limited(1)),
            Some(Some(1))
        );
        assert_eq!(
            reconciliation_limit(HostedCapacityPolicy::Unlimited),
            Some(None)
        );
        assert_eq!(
            reconciliation_limit(HostedCapacityPolicy::Bypass),
            Some(None)
        );
    }
}
