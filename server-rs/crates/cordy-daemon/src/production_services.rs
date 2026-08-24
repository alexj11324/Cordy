//! Concrete daemon-owned production service composition.
//!
//! The provider adapter is mandatory and owns only operations that genuinely
//! require provider/runtime implementations. Workspace membership,
//! registration ordering, runtime-gone recovery, profile refresh, and the
//! reconcile lifecycle remain daemon responsibilities.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordy_protocol::{DaemonHeartbeatAckPayload, RuntimeProfilesChangedPayload};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::activity::DaemonActivity;
use crate::agents_refresh::{AGENT_DISCOVERY_INTERVAL, AGENT_VERSION_REFRESH_INTERVAL};
use crate::client::Client;
use crate::config::{
    Config, DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL, DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL,
    DEFAULT_WORKSPACE_SYNC_INTERVAL, DEFAULT_WORKSPACE_SYNC_MAX_BACKOFF,
};
use crate::daemon_core::DaemonCoreServices;
use crate::health::{
    ActiveRepoCheckoutTask, HealthResponse, RepoCheckoutRegistry, RepoCheckoutRequest,
    REPO_CHECKOUT_LOCK_WAIT_TIMEOUT,
};
use crate::production_stack::{ProductionRuntimeServices, RepoCheckoutFailure};
use crate::reconcile::{ReconcileBroadcaster, WorkspaceChangeSignal};
use crate::registration::{
    enqueue_repo_warmup, BuiltinRefreshReason, RepoWarmupRequest, RuntimeRegistrationService,
    RuntimeRegistrationSource,
};
use crate::repo_state::DaemonRepoState;
use crate::repocache::{is_repo_busy, Cache, Ctx, RepoInfo, WorktreeParams};
use crate::runtime_registry::RuntimeRegistry;
use crate::task_execution::TaskRunOutcome;
use crate::types::{RuntimeExecutionTarget, Task};
use crate::wakeup::jitter_duration;

const REPO_WARMUP_QUEUE_CAPACITY: usize = 64;
const REPO_WARMUP_CONCURRENCY: usize = 2;
const REPO_WARMUP_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait::async_trait]
pub trait ProviderRuntimeAdapter: Send + Sync + 'static {
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
        target: RuntimeExecutionTarget,
        slot: usize,
        activity: Arc<DaemonActivity>,
        repo_state: Arc<DaemonRepoState>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> TaskRunOutcome;

    fn health_snapshot(&self) -> HealthResponse;
}

pub struct DaemonProductionServices<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> {
    config: Arc<Config>,
    client: Arc<Client>,
    provider: Arc<P>,
    registration: RuntimeRegistrationService<R>,
    repo_cache: Arc<Cache>,
    repo_state: Arc<DaemonRepoState>,
    checkout_registry: Arc<RepoCheckoutRegistry>,
    repo_warmups: mpsc::Sender<RepoWarmupRequest>,
    repo_warmup_rx: Mutex<Option<mpsc::Receiver<RepoWarmupRequest>>>,
}

impl<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> DaemonProductionServices<P, R> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        repo_cache: Arc<Cache>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
        provider: Arc<P>,
        registration_source: Arc<R>,
    ) -> Self {
        let repo_state = Arc::new(DaemonRepoState::new());
        let (repo_warmups, repo_warmup_rx) = mpsc::channel(REPO_WARMUP_QUEUE_CAPACITY);
        Self {
            registration: RuntimeRegistrationService::new(
                Arc::clone(&config),
                Arc::clone(&client),
                Arc::clone(&repo_state),
                repo_warmups.clone(),
                registration_source,
            ),
            config,
            client,
            provider,
            repo_cache,
            repo_state,
            checkout_registry,
            repo_warmups,
            repo_warmup_rx: Mutex::new(Some(repo_warmup_rx)),
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

    async fn ensure_repo_ready(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
        repo_url: &str,
    ) -> Result<(), RepoCheckoutFailure> {
        let refresh_lock = self.repo_state.refresh_lock(workspace_id).ok_or_else(|| {
            checkout_failure(
                400,
                format!("workspace is not watched by this daemon: {workspace_id}"),
            )
        })?;
        let cache_hit_on_entry = self.repo_state.is_allowed(workspace_id, repo_url)
            && self.repo_cache.lookup(workspace_id, repo_url).is_some();
        let _guard = tokio::select! {
            () = ctx.cancelled() => return Err(checkout_failure(500, "repo checkout cancelled")),
            guard = refresh_lock.lock() => guard,
        };
        if !cache_hit_on_entry
            && self.repo_state.is_allowed(workspace_id, repo_url)
            && self.repo_cache.lookup(workspace_id, repo_url).is_some()
        {
            return Ok(());
        }

        let response = self
            .client
            .get_workspace_repos(ctx, workspace_id)
            .await
            .map_err(|error| checkout_failure(500, format!("refresh workspace repos: {error}")))?;
        self.repo_state
            .replace_workspace(workspace_id, &response.repos, response.settings);
        if !self.repo_state.is_allowed(workspace_id, repo_url) {
            return Err(checkout_failure(
                400,
                "repository is not configured for this workspace",
            ));
        }
        if self.repo_cache.lookup(workspace_id, repo_url).is_some() {
            return Ok(());
        }

        let repos: Vec<RepoInfo> = response
            .repos
            .into_iter()
            .filter(|repo| !repo.url.is_empty())
            .map(|repo| RepoInfo { url: repo.url })
            .collect();
        match self.repo_cache.sync_ctx(ctx, workspace_id, &repos).await {
            Ok(()) => self.repo_state.set_sync_error(workspace_id, String::new()),
            Err(error) => self
                .repo_state
                .set_sync_error(workspace_id, error.to_string()),
        }
        if self.repo_cache.lookup(workspace_id, repo_url).is_some() {
            return Ok(());
        }
        let sync_error = self.repo_state.last_sync_error(workspace_id);
        if sync_error.is_empty() {
            Err(checkout_failure(
                500,
                "repository is configured but not synced",
            ))
        } else {
            Err(checkout_failure(
                500,
                format!("repository is configured but not synced: {sync_error}"),
            ))
        }
    }

    async fn checkout_repo(
        &self,
        ctx: Ctx,
        active_task: ActiveRepoCheckoutTask,
        request: RepoCheckoutRequest,
    ) -> Result<Value, RepoCheckoutFailure> {
        self.ensure_repo_ready(&ctx, &request.workspace_id, &request.url)
            .await?;
        let reference = if request.r#ref.trim().is_empty() {
            self.repo_state
                .task_default_ref(&request.workspace_id, &request.task_id, &request.url)
        } else {
            request.r#ref.trim().to_string()
        };
        let params = WorktreeParams {
            workspace_id: request.workspace_id.clone(),
            repo_url: request.url.clone(),
            work_dir: request.workdir.into(),
            reference,
            agent_name: active_task.agent_name,
            task_id: request.task_id,
            co_authored_by_enabled: self
                .repo_state
                .co_authored_by_enabled(&request.workspace_id),
            lock_wait_timeout: if request.retry_busy {
                REPO_CHECKOUT_LOCK_WAIT_TIMEOUT
            } else {
                Duration::ZERO
            },
            isolated_git_metadata: request.checkout_mode == "isolated",
        };
        match self.repo_cache.create_worktree_ctx(&ctx, params).await {
            Ok(result) => serde_json::to_value(result)
                .map_err(|error| checkout_failure(500, error.to_string())),
            Err(error) if request.retry_busy && is_repo_busy(&error) => Err(RepoCheckoutFailure {
                status_code: 503,
                message: "repository is busy with another operation; retry later".to_string(),
                retryable_busy: true,
            }),
            Err(error) => Err(checkout_failure(500, error.to_string())),
        }
    }

    fn sync_task_repos(&self, task: &Task) -> TaskRepoRefGuard {
        let candidates =
            self.repo_state
                .register_task_repos(&task.workspace_id, &task.id, &task.repos);
        let repos: Vec<RepoInfo> = candidates
            .into_iter()
            .filter(|url| self.repo_cache.lookup(&task.workspace_id, url).is_none())
            .map(|url| RepoInfo { url })
            .collect();
        enqueue_repo_warmup(&self.repo_warmups, task.workspace_id.clone(), repos);
        TaskRepoRefGuard {
            state: Arc::clone(&self.repo_state),
            workspace_id: task.workspace_id.clone(),
            task_id: task.id.clone(),
        }
    }

    async fn repo_warmup_loop(&self, ctx: Ctx, mut requests: mpsc::Receiver<RepoWarmupRequest>) {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                () = ctx.cancelled() => break,
                completed = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(Err(error)) = completed {
                        tracing::warn!(%error, "repo warmup task failed");
                    }
                }
                request = requests.recv(), if tasks.len() < REPO_WARMUP_CONCURRENCY => {
                    let Some(request) = request else { break };
                    let cache = Arc::clone(&self.repo_cache);
                    let state = Arc::clone(&self.repo_state);
                    let child = ctx.child();
                    tasks.spawn(async move {
                        match cache
                            .sync_ctx(&child, &request.workspace_id, &request.repos)
                            .await
                        {
                            Ok(()) => state.set_sync_error(&request.workspace_id, String::new()),
                            Err(error) => {
                                state.set_sync_error(&request.workspace_id, error.to_string());
                                tracing::warn!(
                                    workspace_id = %request.workspace_id,
                                    %error,
                                    "workspace repo cache warmup failed"
                                );
                            }
                        }
                    });
                }
            }
        }

        let drain = async {
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    tracing::warn!(%error, "repo warmup task failed during shutdown");
                }
            }
        };
        if tokio::time::timeout(REPO_WARMUP_DRAIN_TIMEOUT, drain)
            .await
            .is_err()
        {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            tracing::warn!("repo warmups exceeded shutdown deadline and were aborted");
        }
    }
}

struct TaskRepoRefGuard {
    state: Arc<DaemonRepoState>,
    workspace_id: String,
    task_id: String,
}

impl Drop for TaskRepoRefGuard {
    fn drop(&mut self) {
        self.state
            .clear_task_refs(&self.workspace_id, &self.task_id);
    }
}

fn checkout_failure(status_code: u16, message: impl Into<String>) -> RepoCheckoutFailure {
    RepoCheckoutFailure {
        status_code,
        message: message.into(),
        retryable_busy: false,
    }
}

#[async_trait::async_trait]
impl<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> DaemonCoreServices
    for DaemonProductionServices<P, R>
{
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
        target: RuntimeExecutionTarget,
        slot: usize,
        activity: Arc<DaemonActivity>,
    ) -> TaskRunOutcome {
        // Publish task references synchronously, but do not spend the prepare
        // lease cloning repositories. The owned warmup worker prefetches in
        // parallel; checkout readiness synchronizes a miss on demand.
        let _repo_refs = self.sync_task_repos(&task);
        self.provider
            .run_task(
                ctx,
                task,
                target,
                slot,
                activity,
                Arc::clone(&self.repo_state),
                Arc::clone(&self.checkout_registry),
            )
            .await
    }

    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool {
        self.repo_state
            .all_urls()
            .into_iter()
            .any(|(workspace_id, url)| self.repo_cache.bare_path(&workspace_id, &url) == bare_path)
    }
}

#[async_trait::async_trait]
impl<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> ProductionRuntimeServices
    for DaemonProductionServices<P, R>
{
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
        let repo_warmup_rx = self
            .repo_warmup_rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("repo warmup owner already started"))?;
        tokio::join!(
            self.reconcile_loop(
                ctx.child(),
                reconcile,
                workspace_changes,
                Arc::clone(&registry),
            ),
            self.provider_refresh_loop(ctx.child(), registry),
            self.repo_warmup_loop(ctx, repo_warmup_rx),
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
        self.checkout_repo(ctx, active_task, request).await
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
