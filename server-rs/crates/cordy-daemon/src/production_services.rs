//! Concrete daemon-owned production service composition.
//!
//! The provider adapter is mandatory and owns only operations that genuinely
//! require provider/runtime implementations. Workspace membership,
//! registration ordering, runtime-gone recovery, profile refresh, and the
//! reconcile lifecycle remain daemon responsibilities.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cordy_protocol::{DaemonHeartbeatAckPayload, RuntimeProfilesChangedPayload};
use serde_json::Value;

use crate::activity::DaemonActivity;
use crate::agents_refresh::{AGENT_DISCOVERY_INTERVAL, AGENT_VERSION_REFRESH_INTERVAL};
use crate::client::Client;
use crate::config::{
    Config, DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL, DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL,
    DEFAULT_WORKSPACE_SYNC_INTERVAL, DEFAULT_WORKSPACE_SYNC_MAX_BACKOFF,
};
use crate::daemon_core::DaemonCoreServices;
use crate::health::{ActiveRepoCheckoutTask, HealthResponse, RepoCheckoutRequest};
use crate::production_stack::{ProductionRuntimeServices, RepoCheckoutFailure};
use crate::reconcile::{ReconcileBroadcaster, WorkspaceChangeSignal};
use crate::registration::{
    BuiltinRefreshReason, RuntimeRegistrationService, RuntimeRegistrationSource,
};
use crate::repo_state::DaemonRepoState;
use crate::repocache::{Cache, Ctx};
use crate::runtime_registry::RuntimeRegistry;
use crate::task_execution::TaskRunOutcome;
use crate::types::Task;
use crate::wakeup::jitter_duration;

#[async_trait::async_trait]
pub trait ProviderRuntimeAdapter: RuntimeRegistrationSource {
    async fn handle_non_update_heartbeat_actions(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        runtime_id: String,
        ack: DaemonHeartbeatAckPayload,
    );

    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        provider: String,
        slot: usize,
        activity: Arc<DaemonActivity>,
        repo_state: Arc<DaemonRepoState>,
    ) -> TaskRunOutcome;

    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool;
    fn health_snapshot(&self) -> HealthResponse;

    async fn repo_checkout(
        &self,
        ctx: Ctx,
        active_task: ActiveRepoCheckoutTask,
        request: RepoCheckoutRequest,
    ) -> Result<Value, RepoCheckoutFailure>;
}

pub struct DaemonProductionServices<P: ProviderRuntimeAdapter> {
    config: Arc<Config>,
    client: Arc<Client>,
    provider: Arc<P>,
    registration: RuntimeRegistrationService<P>,
    repo_state: Arc<DaemonRepoState>,
}

impl<P: ProviderRuntimeAdapter> DaemonProductionServices<P> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        repo_cache: Arc<Cache>,
        provider: Arc<P>,
    ) -> Self {
        let repo_state = Arc::new(DaemonRepoState::new());
        Self {
            registration: RuntimeRegistrationService::new(
                Arc::clone(&config),
                Arc::clone(&client),
                repo_cache,
                Arc::clone(&repo_state),
                Arc::clone(&provider),
            ),
            config,
            client,
            provider,
            repo_state,
        }
    }

    fn sync_base_interval(&self, registry: &RuntimeRegistry) -> Duration {
        if registry.runtime_ids().is_empty() {
            DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL
        } else if self.client.uses_legacy_workspace_endpoint() {
            DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL
        } else {
            DEFAULT_WORKSPACE_SYNC_INTERVAL
        }
    }

    async fn reconcile_loop(
        &self,
        ctx: Ctx,
        reconcile: Arc<ReconcileBroadcaster>,
        workspace_changes: Arc<WorkspaceChangeSignal>,
        registry: Arc<RuntimeRegistry>,
    ) {
        let mut reconcile_snapshot = reconcile.notify();
        let mut failures = 0u32;
        loop {
            let base = self.sync_base_interval(&registry);
            let delay = workspace_sync_backoff(base, failures);
            let reconcile_profiles = tokio::select! {
                () = ctx.cancelled() => return,
                () = reconcile_snapshot.recv() => {
                    reconcile_snapshot = reconcile.notify();
                    true
                }
                changed = workspace_changes.recv() => {
                    if changed.is_none() { return; }
                    false
                }
                _ = tokio::time::sleep(jitter_duration(delay)) => false,
            };
            match self
                .registration
                .sync_once(ctx.child(), &registry, reconcile_profiles)
                .await
            {
                Ok(()) => failures = 0,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    tracing::debug!(%error, failures, "workspace sync failed");
                }
            }
        }
    }

    async fn provider_refresh_loop(&self, ctx: Ctx, registry: Arc<RuntimeRegistry>) {
        let now = tokio::time::Instant::now();
        let mut discovery =
            tokio::time::interval_at(now + AGENT_DISCOVERY_INTERVAL, AGENT_DISCOVERY_INTERVAL);
        let mut versions = tokio::time::interval_at(
            now + AGENT_VERSION_REFRESH_INTERVAL,
            AGENT_VERSION_REFRESH_INTERVAL,
        );
        discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        versions.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let reason = tokio::select! {
                () = ctx.cancelled() => return,
                _ = discovery.tick() => BuiltinRefreshReason::Discovery,
                _ = versions.tick() => BuiltinRefreshReason::Version,
            };
            if let Err(error) = self
                .registration
                .refresh_builtins_once(ctx.child(), &registry, reason)
                .await
            {
                tracing::debug!(?reason, %error, "built-in runtime refresh round failed");
            }
        }
    }
}

#[async_trait::async_trait]
impl<P: ProviderRuntimeAdapter> DaemonCoreServices for DaemonProductionServices<P> {
    async fn handle_runtime_gone(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        runtime_id: String,
    ) {
        if let Err(error) = self
            .registration
            .recover_runtime_gone(ctx, &registry, &runtime_id)
            .await
        {
            tracing::warn!(%runtime_id, %error, "runtime-gone recovery failed");
        }
    }

    async fn refresh_workspace_runtime_profiles(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        payload: RuntimeProfilesChangedPayload,
    ) {
        if let Err(error) = self
            .registration
            .refresh_workspace(ctx, &registry, &payload.workspace_id)
            .await
        {
            tracing::debug!(workspace_id = %payload.workspace_id, %error, "runtime profile refresh failed");
        }
    }

    async fn handle_non_update_heartbeat_actions(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        runtime_id: String,
        ack: DaemonHeartbeatAckPayload,
    ) {
        self.provider
            .handle_non_update_heartbeat_actions(ctx, registry, runtime_id, ack)
            .await;
    }

    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        provider: String,
        slot: usize,
        activity: Arc<DaemonActivity>,
    ) -> TaskRunOutcome {
        self.provider
            .run_task(
                ctx,
                task,
                provider,
                slot,
                activity,
                Arc::clone(&self.repo_state),
            )
            .await
    }

    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool {
        self.provider.repo_bare_path_is_live(bare_path)
    }
}

#[async_trait::async_trait]
impl<P: ProviderRuntimeAdapter> ProductionRuntimeServices for DaemonProductionServices<P> {
    async fn preflight(&self, ctx: Ctx, registry: Arc<RuntimeRegistry>) -> anyhow::Result<()> {
        self.registration.sync_once(ctx, &registry, false).await
    }

    async fn run_reconcile(
        &self,
        ctx: Ctx,
        reconcile: Arc<ReconcileBroadcaster>,
        workspace_changes: Arc<WorkspaceChangeSignal>,
        registry: Arc<RuntimeRegistry>,
    ) -> anyhow::Result<()> {
        tokio::join!(
            self.reconcile_loop(
                ctx.child(),
                reconcile,
                workspace_changes,
                Arc::clone(&registry),
            ),
            self.provider_refresh_loop(ctx, registry),
        );
        Ok(())
    }

    fn health_snapshot(&self) -> HealthResponse {
        let mut snapshot = self.provider.health_snapshot();
        snapshot.profile = self.config.profile.clone();
        snapshot
    }

    async fn repo_checkout(
        &self,
        ctx: Ctx,
        active_task: ActiveRepoCheckoutTask,
        request: RepoCheckoutRequest,
    ) -> Result<Value, RepoCheckoutFailure> {
        self.provider.repo_checkout(ctx, active_task, request).await
    }
}

fn workspace_sync_backoff(base: Duration, failures: u32) -> Duration {
    let maximum = if base == DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL {
        DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL
    } else {
        DEFAULT_WORKSPACE_SYNC_MAX_BACKOFF
    };
    let mut interval = base;
    for _ in 0..failures {
        interval = interval.saturating_mul(2).min(maximum);
        if interval == maximum {
            break;
        }
    }
    interval
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_backoff_matches_bootstrap_and_steady_caps() {
        assert_eq!(
            workspace_sync_backoff(DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL, 10),
            DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL
        );
        assert_eq!(
            workspace_sync_backoff(DEFAULT_WORKSPACE_SYNC_INTERVAL, 10),
            DEFAULT_WORKSPACE_SYNC_MAX_BACKOFF
        );
        assert_eq!(
            workspace_sync_backoff(DEFAULT_WORKSPACE_SYNC_INTERVAL, 0),
            DEFAULT_WORKSPACE_SYNC_INTERVAL
        );
    }
}
