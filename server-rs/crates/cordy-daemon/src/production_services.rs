//! Concrete daemon-owned production service composition.
//!
//! The provider adapter is mandatory and owns only operations that genuinely
//! require provider/runtime implementations. Workspace membership,
//! registration ordering, runtime-gone recovery, profile refresh, and the
//! reconcile lifecycle remain daemon responsibilities.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordy_agent::{
    AntigravityBackend, AntigravityConfig, BackendConfig, CatalogCache, CodebuddyBackend,
    CodebuddyConfig, DimBackend, DimConfig, DshBackend, DshConfig, GrokBackend, GrokConfig,
    HermesBackend, HermesConfig, KimiBackend, KimiConfig, KiroBackend, KiroConfig, QoderBackend,
    QoderConfig, ReasonixBackend, ReasonixConfig, RuntimeCommand, TraecliBackend, TraecliConfig,
};
use cordy_protocol::{DaemonHeartbeatAckPayload, RuntimeProfilesChangedPayload};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::activity::DaemonActivity;
use crate::agents_refresh::{AGENT_DISCOVERY_INTERVAL, AGENT_VERSION_REFRESH_INTERVAL};
use crate::client::{request_status_code, Client};
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
use crate::provider_registration::RuntimeLaunchRegistry;
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
const ACP_MODEL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const RUNTIME_REPORT_BACKOFFS: &[Duration] = &[
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

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
        runtime: ProviderRuntimeContext,
    ) -> TaskRunOutcome;

    fn health_snapshot(&self) -> HealthResponse;
}

/// Shared owners for one provider task execution.
///
/// These values are deliberately bundled so a production adapter cannot
/// accidentally combine a client from one daemon instance with repository or
/// launch state from another. The launch registry is the same registry fed by
/// [`ProviderRegistrationSource`](crate::provider_registration::ProviderRegistrationSource)
/// and therefore contains only accepted workspace registration state.
#[derive(Clone)]
pub struct ProviderRuntimeContext {
    client: Arc<Client>,
    launch_registry: Arc<RuntimeLaunchRegistry>,
    activity: Arc<DaemonActivity>,
    repo_state: Arc<DaemonRepoState>,
    checkout_registry: Arc<RepoCheckoutRegistry>,
}

impl ProviderRuntimeContext {
    pub(crate) fn new(
        client: Arc<Client>,
        launch_registry: Arc<RuntimeLaunchRegistry>,
        activity: Arc<DaemonActivity>,
        repo_state: Arc<DaemonRepoState>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> Self {
        Self {
            client,
            launch_registry,
            activity,
            repo_state,
            checkout_registry,
        }
    }

    /// Authenticated daemon client for task transcript, usage, and lifecycle
    /// callbacks. The returned `Arc` is the shared production owner.
    pub fn client(&self) -> Arc<Client> {
        Arc::clone(&self.client)
    }

    /// Resolves a launch only from the registration state accepted for this
    /// workspace and target.
    pub fn launch_registry(&self) -> Arc<RuntimeLaunchRegistry> {
        Arc::clone(&self.launch_registry)
    }

    /// Converts one accepted launch into the provider crate's command
    /// contract. The caller supplies only task-scoped environment values;
    /// command path and fixed arguments always come from the workspace
    /// registration state, never from task payload or process environment.
    pub fn backend_config(
        &self,
        workspace_id: &str,
        target: &RuntimeExecutionTarget,
        env: BTreeMap<String, String>,
    ) -> anyhow::Result<BackendConfig> {
        let launch = self.resolve_launch(workspace_id, target)?;
        Ok(BackendConfig {
            command: RuntimeCommand::new(launch.command_path, launch.fixed_args),
            env,
            builtin_runtime: target.profile_id.is_empty(),
        })
    }

    pub fn backend_config_with_prefix(
        &self,
        workspace_id: &str,
        target: &RuntimeExecutionTarget,
        env: BTreeMap<String, String>,
        prefix: Vec<String>,
    ) -> anyhow::Result<BackendConfig> {
        let launch = self.resolve_launch(workspace_id, target)?;
        Ok(BackendConfig {
            command: RuntimeCommand::new(launch.command_path, prefix),
            env,
            builtin_runtime: target.profile_id.is_empty(),
        })
    }

    fn resolve_launch(
        &self,
        workspace_id: &str,
        target: &RuntimeExecutionTarget,
    ) -> anyhow::Result<crate::provider_registration::RuntimeLaunchSpec> {
        let launch = self
            .launch_registry
            .resolve(workspace_id, target)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no accepted launch registered for workspace {workspace_id:?} and provider {}",
                    target.provider
                )
            })?;
        anyhow::ensure!(
            !launch.command_path.trim().is_empty(),
            "accepted launch for provider {} has no executable path",
            target.provider
        );
        Ok(launch)
    }

    /// Process-wide activity state used to coordinate execution with update
    /// and garbage-collection barriers.
    pub fn activity(&self) -> Arc<DaemonActivity> {
        Arc::clone(&self.activity)
    }

    /// Daemon-owned repository authorization and task-reference state.
    pub fn repo_state(&self) -> Arc<DaemonRepoState> {
        Arc::clone(&self.repo_state)
    }

    /// Task-bound localhost checkout authorization registry.
    pub fn checkout_registry(&self) -> Arc<RepoCheckoutRegistry> {
        Arc::clone(&self.checkout_registry)
    }
}

pub struct DaemonProductionServices<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> {
    config: Arc<Config>,
    client: Arc<Client>,
    provider: Arc<P>,
    registration: RuntimeRegistrationService<R>,
    repo_cache: Arc<Cache>,
    repo_state: Arc<DaemonRepoState>,
    checkout_registry: Arc<RepoCheckoutRegistry>,
    launch_registry: Arc<RuntimeLaunchRegistry>,
    repo_warmups: mpsc::Sender<RepoWarmupRequest>,
    repo_warmup_rx: Mutex<Option<mpsc::Receiver<RepoWarmupRequest>>>,
    model_cache: Arc<CatalogCache>,
}

impl<P: ProviderRuntimeAdapter, R: RuntimeRegistrationSource> DaemonProductionServices<P, R> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        repo_cache: Arc<Cache>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
        launch_registry: Arc<RuntimeLaunchRegistry>,
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
            launch_registry,
            repo_warmups,
            repo_warmup_rx: Mutex::new(Some(repo_warmup_rx)),
            model_cache: Arc::new(CatalogCache::default()),
        }
    }

    /// Completes Rust-backed model-list contracts from the same accepted launch
    /// identity used by task execution. Custom profile runtimes therefore
    /// discover against their pinned executable and fixed prefix.
    async fn handle_acp_model_list(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        runtime_id: &str,
        request_id: &str,
    ) -> bool {
        let Some(target) = registry.execution_target_for_runtime(runtime_id) else {
            return false;
        };
        if !matches!(
            target.provider.as_str(),
            "hermes"
                | "kimi"
                | "kiro"
                | "reasonix"
                | "grok"
                | "qoder"
                | "qoderclicn"
                | "traecli"
                | "antigravity"
                | "codebuddy"
                | "dsh"
                | "qwen"
                | "qwenpaw"
                | "mcode"
                | "dim"
        ) {
            return false;
        }

        let result = async {
            let workspace_id = registry
                .workspace_for_runtime(runtime_id)
                .ok_or_else(|| anyhow::anyhow!("runtime {runtime_id} has no workspace"))?;
            let launch = self
                .launch_registry
                .resolve(&workspace_id, &target)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no accepted launch registered for workspace {workspace_id:?} and provider {}",
                        target.provider
                    )
                })?;
            anyhow::ensure!(
                !launch.command_path.trim().is_empty(),
                "accepted {} launch has no executable path",
                target.provider
            );
            let command = RuntimeCommand::new(launch.command_path, launch.fixed_args);
            let runtime_scope = format!(
                "{}\0workspace={workspace_id}\0runtime={runtime_id}\0profile={}",
                target.provider,
                target.profile_id
            );
            let catalog = match target.provider.as_str() {
                "hermes" => HermesBackend::new(HermesConfig {
                    command,
                    env: BTreeMap::new(),
                    builtin_runtime: target.profile_id.is_empty(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "kimi" => KimiBackend::new(KimiConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime_result(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await
                .map_err(|error| anyhow::anyhow!(error))?,
                "kiro" => KiroBackend::new(KiroConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "reasonix" => ReasonixBackend::new(ReasonixConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "grok" => GrokBackend::new(GrokConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "qoder" | "qoderclicn" => {
                    let default_command = if target.provider == "qoderclicn" {
                        "qoderclicn"
                    } else {
                        "qodercli"
                    };
                    QoderBackend::new(QoderConfig {
                        command,
                        env: BTreeMap::new(),
                        default_command: default_command.to_string(),
                        provider: target.provider.clone(),
                        ..QoderConfig::default()
                    })
                    .discover_models_for_runtime(
                        &runtime_scope,
                        &self.model_cache,
                        ctx.token().clone(),
                        ACP_MODEL_DISCOVERY_TIMEOUT,
                    )
                    .await
                }
                "traecli" => TraecliBackend::new(TraecliConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "antigravity" => AntigravityBackend::new(AntigravityConfig {
                    command,
                    env: BTreeMap::new(),
                    catalog_cache: Arc::clone(&self.model_cache),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "codebuddy" => CodebuddyBackend::new(CodebuddyConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "dsh" => DshBackend::new(DshConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                "qwen" | "qwenpaw" | "mcode" => cordy_agent::Catalog::default(),
                "dim" => DimBackend::new(DimConfig {
                    command,
                    env: BTreeMap::new(),
                })
                .discover_models_for_runtime(
                    &runtime_scope,
                    &self.model_cache,
                    ctx.token().clone(),
                    ACP_MODEL_DISCOVERY_TIMEOUT,
                )
                .await,
                _ => unreachable!("provider filtered above"),
            };
            Ok::<_, anyhow::Error>(model_list_completed_payload(&target.provider, catalog))
        }
        .await;

        if ctx.err().is_some() {
            tracing::debug!(
                %runtime_id,
                %request_id,
                provider = %target.provider,
                "ACP model-list action cancelled; leaving pending action unhandled"
            );
            return false;
        }
        let payload = result.unwrap_or_else(|error| model_list_failed_payload(error));
        self.report_model_list_result_with_retry(
            &ctx,
            &target.provider,
            runtime_id,
            request_id,
            payload,
        )
        .await
    }

    async fn report_model_list_result_with_retry(
        &self,
        ctx: &Ctx,
        provider: &str,
        runtime_id: &str,
        request_id: &str,
        payload: Value,
    ) -> bool {
        for (attempt, wait) in RUNTIME_REPORT_BACKOFFS.iter().copied().enumerate() {
            if !wait.is_zero() {
                tokio::select! {
                    () = ctx.cancelled() => return false,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            match self
                .client
                .report_model_list_result(ctx, runtime_id, request_id, payload.clone())
                .await
            {
                Ok(()) => return true,
                Err(error) => {
                    let permanent = request_status_code(&error)
                        .is_some_and(|status| (400..500).contains(&status));
                    if permanent || attempt + 1 == RUNTIME_REPORT_BACKOFFS.len() {
                        tracing::error!(
                            %runtime_id,
                            %request_id,
                            %provider,
                            error = %error,
                            "ACP model-list report failed"
                        );
                        return false;
                    }
                    tracing::warn!(
                        %runtime_id,
                        %request_id,
                        %provider,
                        error = %error,
                        "ACP model-list report failed; retrying"
                    );
                }
            }
        }
        false
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
        let _guard = tokio::select! {
            () = ctx.cancelled() => return Err(checkout_failure(500, "repo checkout cancelled")),
            guard = refresh_lock.lock() => guard,
        };
        // Re-check under the workspace refresh lock. A warm authorized cache
        // is already complete and must remain usable during transient server
        // outages; a concurrent refresh may also have filled a prior miss
        // while this request waited for the lock.
        if self.repo_state.is_allowed(workspace_id, repo_url)
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

        // A project-only repository is authorized by the active task but is
        // intentionally absent from the workspace repo response. Include the
        // exact authorized miss so correctness does not depend on the
        // best-effort warmup queue winning the race with first checkout.
        let repos = repo_sync_candidates(response.repos, repo_url);
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

fn model_list_completed_payload(provider: &str, catalog: cordy_agent::Catalog) -> Value {
    json!({
        "status": "completed",
        "models": catalog.models,
        "supported": cordy_agent::registry::model_selection_supported(provider),
        "fallback": catalog.fallback,
    })
}

fn model_list_failed_payload(error: impl std::fmt::Display) -> Value {
    json!({
        "status": "failed",
        "error": error.to_string(),
    })
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

fn repo_sync_candidates(repos: Vec<crate::types::RepoData>, requested: &str) -> Vec<RepoInfo> {
    let mut urls = repos
        .into_iter()
        .map(|repo| repo.url.trim().to_string())
        .filter(|url| !url.is_empty())
        .collect::<BTreeSet<_>>();
    let requested = requested.trim();
    if !requested.is_empty() {
        urls.insert(requested.to_string());
    }
    urls.into_iter().map(|url| RepoInfo { url }).collect()
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
        mut ack: DaemonHeartbeatAckPayload,
    ) {
        if let Some(request) = ack.pending_model_list.clone() {
            if self
                .handle_acp_model_list(ctx.child(), Arc::clone(&registry), &runtime_id, &request.id)
                .await
            {
                ack.pending_model_list = None;
            }
        }
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
                ProviderRuntimeContext::new(
                    Arc::clone(&self.client),
                    Arc::clone(&self.launch_registry),
                    activity,
                    Arc::clone(&self.repo_state),
                    Arc::clone(&self.checkout_registry),
                ),
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

    async fn flush_runtime_cleanup(&self, ctx: Ctx) -> anyhow::Result<()> {
        self.registration.flush_deregistrations(&ctx).await
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

    #[test]
    fn cold_miss_sync_includes_authorized_task_repo_not_in_workspace_response() {
        let repos = repo_sync_candidates(
            vec![
                crate::types::RepoData {
                    url: " https://example.test/workspace.git ".to_string(),
                    ..crate::types::RepoData::default()
                },
                crate::types::RepoData {
                    url: "https://example.test/workspace.git".to_string(),
                    ..crate::types::RepoData::default()
                },
            ],
            "https://example.test/project.git",
        );

        assert_eq!(
            repos.into_iter().map(|repo| repo.url).collect::<Vec<_>>(),
            vec![
                "https://example.test/project.git".to_string(),
                "https://example.test/workspace.git".to_string(),
            ]
        );
    }

    #[test]
    fn provider_runtime_context_keeps_shared_daemon_owners() {
        let client = Arc::new(Client::new("https://example.test"));
        let launch_registry = Arc::new(RuntimeLaunchRegistry::default());
        let activity = DaemonActivity::new();
        let repo_state = Arc::new(DaemonRepoState::new());
        let checkout_registry = Arc::new(RepoCheckoutRegistry::default());
        let context = ProviderRuntimeContext::new(
            Arc::clone(&client),
            Arc::clone(&launch_registry),
            Arc::clone(&activity),
            Arc::clone(&repo_state),
            Arc::clone(&checkout_registry),
        );

        assert!(Arc::ptr_eq(&context.client(), &client));
        assert!(Arc::ptr_eq(&context.launch_registry(), &launch_registry));
        assert!(Arc::ptr_eq(&context.activity(), &activity));
        assert!(Arc::ptr_eq(&context.repo_state(), &repo_state));
        assert!(Arc::ptr_eq(
            &context.checkout_registry(),
            &checkout_registry
        ));
    }

    #[test]
    fn provider_runtime_context_builds_backend_config_from_accepted_launch() {
        let client = Arc::new(Client::new("https://example.test"));
        let launch_registry = Arc::new(RuntimeLaunchRegistry::default());
        let target = RuntimeExecutionTarget {
            provider: "codex".to_string(),
            profile_id: String::new(),
        };
        launch_registry.replace_builtins(
            "workspace-1",
            vec![crate::provider_registration::RuntimeLaunchSpec {
                target: target.clone(),
                display_name: "Codex".to_string(),
                command_path: "/opt/codex".to_string(),
                fixed_args: vec!["--profile".to_string(), "cordy".to_string()],
                version: "1.0.0".to_string(),
            }],
        );
        let context = ProviderRuntimeContext::new(
            client,
            Arc::clone(&launch_registry),
            DaemonActivity::new(),
            Arc::new(DaemonRepoState::new()),
            Arc::new(RepoCheckoutRegistry::default()),
        );
        let config = context
            .backend_config(
                "workspace-1",
                &target,
                BTreeMap::from([("CORDY_TASK_ID".to_string(), "task-1".to_string())]),
            )
            .expect("accepted launch must resolve");
        assert_eq!(config.command.path, "/opt/codex");
        assert_eq!(
            config.command.prefix,
            vec!["--profile".to_string(), "cordy".to_string()]
        );
        assert_eq!(
            config.env.get("CORDY_TASK_ID").map(String::as_str),
            Some("task-1")
        );
        assert!(config.builtin_runtime);
    }

    #[test]
    fn provider_runtime_context_rejects_unregistered_launch() {
        let context = ProviderRuntimeContext::new(
            Arc::new(Client::new("https://example.test")),
            Arc::new(RuntimeLaunchRegistry::default()),
            DaemonActivity::new(),
            Arc::new(DaemonRepoState::new()),
            Arc::new(RepoCheckoutRegistry::default()),
        );
        let error = context
            .backend_config(
                "workspace-1",
                &RuntimeExecutionTarget {
                    provider: "codex".to_string(),
                    profile_id: String::new(),
                },
                BTreeMap::new(),
            )
            .expect_err("unregistered launch must fail closed");
        assert!(error.to_string().contains("no accepted launch"));
    }
}
