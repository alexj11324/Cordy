//! Owned production daemon supervisor and localhost health server.
//!
//! The future CLI binary constructs this stack with a real
//! [`ProductionRuntimeServices`] implementation. Provider execution remains a
//! mandatory dependency: this module has no fallback runner or no-op service.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::activity::DaemonActivity;
use crate::auth_lifecycle::{renew_token_once, token_renewal_loop};
use crate::auto_update::{auto_update_loop, AutoUpdateProbes};
use crate::bootstrap::{BootstrapClock, DaemonStackExit, SystemBootstrapClock};
use crate::client::Client;
use crate::config::Config;
use crate::control_lifecycle::{run_daemon_control, ControlEventConsumer};
use crate::daemon_core::{DaemonCoreHost, DaemonCoreServices};
use crate::gc::gc_loop;
use crate::health::{
    authorize_repo_checkout_workdir, ActiveRepoCheckoutTask, HealthResponse, RepoCheckoutRegistry,
    RepoCheckoutRequest,
};
use crate::manager::{ControlEvent, DaemonControl};
use crate::reconcile::{ReconcileBroadcaster, WorkspaceChangeSignal};
use crate::repocache::{Cache, CancelCause, Ctx};
use crate::task_execution::{TaskExecutionConfig, TaskExecutionOrchestrator};
use crate::update_executor::UpdateExecutor;

const TASK_WAKEUP_CAPACITY: usize = 256;
const OWNED_DRAIN_TIMEOUT: Duration = Duration::from_secs(35);
const DEREGISTER_TIMEOUT: Duration = Duration::from_secs(5);
const CHECKOUT_MODE_ISOLATED: &str = "isolated";

/// Provider/runtime operations that cannot be implemented by the daemon
/// control plane itself. A production stack cannot be constructed without a
/// concrete implementation.
#[async_trait::async_trait]
pub trait ProductionRuntimeServices: DaemonCoreServices {
    /// Initial workspace sync, agent probing, and runtime registration.
    /// Authentication renewal is owned by the production stack and completes
    /// its best-effort first attempt before this method is called. Returned
    /// IDs become the authoritative control set.
    async fn preflight(&self, ctx: Ctx) -> anyhow::Result<Vec<String>>;

    /// Owns workspace consistency and agent-discovery reconciliation. It must
    /// observe both signals and return only after `ctx` cancellation.
    async fn run_reconcile(
        &self,
        ctx: Ctx,
        reconcile: Arc<ReconcileBroadcaster>,
        workspace_changes: Arc<WorkspaceChangeSignal>,
    ) -> anyhow::Result<()>;

    /// Provider-owned health fields: workspaces, agents, skipped-agent
    /// diagnostics, and task execution counters not represented by
    /// `DaemonActivity`.
    fn health_snapshot(&self) -> HealthResponse;

    /// Performs the real ensure-repo/default-ref/worktree operation after the
    /// stack has authenticated and normalized the request.
    async fn repo_checkout(
        &self,
        ctx: Ctx,
        active_task: ActiveRepoCheckoutTask,
        request: RepoCheckoutRequest,
    ) -> Result<Value, RepoCheckoutFailure>;
}

#[derive(Debug)]
pub struct RepoCheckoutFailure {
    pub status_code: u16,
    pub message: String,
    pub retryable_busy: bool,
}

impl std::fmt::Display for RepoCheckoutFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RepoCheckoutFailure {}

pub struct DaemonProductionStack<S: ProductionRuntimeServices> {
    config: Arc<Config>,
    client: Arc<Client>,
    repo_cache: Arc<Cache>,
    services: Arc<S>,
    checkout_registry: Arc<RepoCheckoutRegistry>,
    update_executor: Arc<UpdateExecutor>,
    clock: Arc<dyn BootstrapClock>,
}

impl<S: ProductionRuntimeServices> DaemonProductionStack<S> {
    /// Constructs the real stack. Update detection is part of construction so
    /// no caller can accidentally install a pretend update executor.
    pub async fn new(
        config: Config,
        client: Arc<Client>,
        repo_cache: Arc<Cache>,
        services: Arc<S>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
    ) -> anyhow::Result<Self> {
        Self::new_with_clock(
            config,
            client,
            repo_cache,
            services,
            checkout_registry,
            Arc::new(SystemBootstrapClock),
        )
        .await
    }

    pub async fn new_with_clock(
        config: Config,
        client: Arc<Client>,
        repo_cache: Arc<Cache>,
        services: Arc<S>,
        checkout_registry: Arc<RepoCheckoutRegistry>,
        clock: Arc<dyn BootstrapClock>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            (1..=u16::MAX as i32).contains(&config.health_port),
            "health port must be between 1 and 65535"
        );
        anyhow::ensure!(
            config.max_concurrent_tasks > 0,
            "max_concurrent_tasks must be greater than zero"
        );
        let update_executor = Arc::new(UpdateExecutor::detect().await?);
        Ok(Self {
            config: Arc::new(config),
            client,
            repo_cache,
            services,
            checkout_registry,
            update_executor,
            clock,
        })
    }

    /// Runs one complete daemon lifetime and returns a restart target only
    /// after every owned loop has observed cancellation and runtimes have had
    /// their bounded deregistration attempt.
    pub async fn run(
        &self,
        bootstrap_shutdown: CancellationToken,
    ) -> anyhow::Result<DaemonStackExit> {
        let root_ctx = Ctx::new();
        let ready = Arc::new(AtomicBool::new(false));
        let started_at = self.clock.now();
        let activity = DaemonActivity::new();
        let reconcile = Arc::new(ReconcileBroadcaster::new());
        let workspace_changes = Arc::new(WorkspaceChangeSignal::new());
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel::<ControlEvent>();
        let (task_wakeups_tx, task_wakeups_rx) = tokio::sync::mpsc::channel(TASK_WAKEUP_CAPACITY);
        let control = DaemonControl::new(
            Arc::clone(&self.client),
            self.config.server_base_url.clone(),
            self.config.daemon_id.clone(),
            self.config.heartbeat_interval,
            events_tx,
        );
        let host = Arc::new(DaemonCoreHost::new(
            Arc::clone(&self.config),
            Arc::clone(&self.client),
            Arc::clone(&self.repo_cache),
            Arc::clone(&self.services),
            Arc::clone(&self.update_executor),
            Arc::clone(&activity),
            root_ctx.clone(),
        ));

        // Bind before preflight so liveness reports `starting` during slow
        // auth/agent discovery, matching the Go daemon contract.
        let listener = TcpListener::bind(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            self.config.health_port as u16,
        ))
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "another daemon is already running on 127.0.0.1:{}: {error}",
                self.config.health_port
            )
        })?;
        let health_state = Arc::new(HealthState {
            config: Arc::clone(&self.config),
            services: Arc::clone(&self.services),
            host: Arc::clone(&host),
            repo_cache: Arc::clone(&self.repo_cache),
            checkout_registry: Arc::clone(&self.checkout_registry),
            ready: Arc::clone(&ready),
            root_ctx: root_ctx.clone(),
            started_at,
            clock: Arc::clone(&self.clock),
        });
        let mut health_task = spawn_health_server(listener, health_state, root_ctx.clone());
        let bridge_ctx = root_ctx.clone();
        let bridge = tokio::spawn(async move {
            bootstrap_shutdown.cancelled().await;
            bridge_ctx.cancel_with(CancelCause::Shutdown);
        });

        // Go renews the PAT synchronously before the first workspace request.
        // Renewal itself is best-effort; preflight remains the readiness gate.
        renew_token_once(&self.client, &self.config.profile, &root_ctx.child()).await;
        let runtime_ids = match self.services.preflight(root_ctx.child()).await {
            Ok(runtime_ids) => runtime_ids,
            Err(error) => {
                root_ctx.cancel_with(CancelCause::Shutdown);
                bridge.abort();
                stop_health_task(&mut health_task).await;
                return Err(error).context("daemon preflight failed");
            }
        };
        control.set_runtime_ids(runtime_ids);
        let registered_control = Arc::clone(&control);

        let consumer = Arc::new(ControlEventConsumer::new(
            Arc::clone(&host),
            task_wakeups_tx,
            Arc::clone(&reconcile),
            Arc::clone(&workspace_changes),
        ));
        let orchestrator = match TaskExecutionOrchestrator::new(
            TaskExecutionConfig {
                max_concurrent_tasks: self.config.max_concurrent_tasks as usize,
                poll_interval: self.config.poll_interval,
                cancel_poll_interval: Duration::ZERO,
                workspaces_root: self.config.workspaces_root.clone(),
            },
            Arc::clone(&self.client),
            Arc::clone(&control),
            Arc::clone(&host),
            Arc::clone(&reconcile),
            Arc::clone(&activity),
        ) {
            Ok(orchestrator) => Arc::new(orchestrator),
            Err(error) => {
                root_ctx.cancel_with(CancelCause::Shutdown);
                bridge.abort();
                stop_health_task(&mut health_task).await;
                self.deregister_runtimes(registered_control.runtime_ids())
                    .await;
                return Err(error).context("construct daemon task orchestrator");
            }
        };

        let mut owners = JoinSet::new();
        let renewal_ctx = root_ctx.child();
        let renewal_client = Arc::clone(&self.client);
        let renewal_profile = self.config.profile.clone();
        owners.spawn(async move {
            token_renewal_loop(renewal_client, renewal_profile, renewal_ctx).await;
        });
        let control_ctx = root_ctx.child();
        let control_root = root_ctx.clone();
        owners.spawn(async move {
            run_daemon_control(control_ctx.clone(), control, consumer, events_rx).await;
            if control_ctx.err().is_none() {
                tracing::error!("daemon control owner stopped unexpectedly");
                control_root.cancel_with(CancelCause::Shutdown);
            }
        });
        let task_ctx = root_ctx.child();
        let task_root = root_ctx.clone();
        owners.spawn(async move {
            orchestrator.run(task_ctx.clone(), task_wakeups_rx).await;
            if task_ctx.err().is_none() {
                tracing::error!("daemon task execution owner stopped unexpectedly");
                task_root.cancel_with(CancelCause::Shutdown);
            }
        });
        let reconcile_ctx = root_ctx.child();
        let reconcile_root = root_ctx.clone();
        let reconcile_services = Arc::clone(&self.services);
        let reconcile_signal = Arc::clone(&reconcile);
        let workspace_signal = Arc::clone(&workspace_changes);
        owners.spawn(async move {
            let result = reconcile_services
                .run_reconcile(reconcile_ctx.clone(), reconcile_signal, workspace_signal)
                .await;
            if reconcile_ctx.err().is_none() {
                match result {
                    Ok(()) => tracing::error!("daemon reconcile owner stopped unexpectedly"),
                    Err(error) => tracing::error!(%error, "daemon reconcile owner failed"),
                }
                reconcile_root.cancel_with(CancelCause::Shutdown);
            }
        });
        let gc_ctx = root_ctx.child();
        let gc_host = Arc::clone(&host);
        owners.spawn(async move { gc_loop(gc_host.as_ref(), &gc_ctx).await });
        let update_ctx = root_ctx.child();
        let update_host = Arc::clone(&host);
        owners.spawn(async move {
            auto_update_loop(update_host.as_ref(), &update_ctx, AutoUpdateProbes::real()).await;
        });

        ready.store(true, Ordering::Release);
        let mut owner_failure = None;
        let health_failure = loop {
            tokio::select! {
                () = root_ctx.cancelled() => break None,
                result = &mut health_task => {
                    root_ctx.cancel_with(CancelCause::Shutdown);
                    break Some(match result {
                        Ok(Ok(())) => anyhow::anyhow!("daemon health server stopped unexpectedly"),
                        Ok(Err(error)) => error.context("daemon health server failed"),
                        Err(error) => anyhow::Error::new(error).context("daemon health task join failed"),
                    });
                }
                result = owners.join_next() => {
                    match result {
                        Some(Ok(())) => {}
                        Some(Err(error)) if root_ctx.err().is_none() => {
                            root_ctx.cancel_with(CancelCause::Shutdown);
                            owner_failure = Some(anyhow::Error::new(error).context("daemon owned task failed"));
                        }
                        Some(Err(error)) => tracing::warn!(%error, "daemon owned task join failed during shutdown"),
                        None => {
                            if root_ctx.err().is_none() {
                                root_ctx.cancel_with(CancelCause::Shutdown);
                                owner_failure = Some(anyhow::anyhow!("all daemon owned tasks stopped unexpectedly"));
                            }
                            break None;
                        }
                    }
                }
            }
        };
        ready.store(false, Ordering::Release);
        bridge.abort();
        let _ = bridge.await;

        let drain = async {
            while let Some(result) = owners.join_next().await {
                if let Err(error) = result {
                    tracing::warn!(%error, "daemon owned task join failed");
                }
            }
        };
        if tokio::time::timeout(OWNED_DRAIN_TIMEOUT, drain)
            .await
            .is_err()
        {
            owners.abort_all();
            while owners.join_next().await.is_some() {}
            tracing::warn!("daemon owned tasks exceeded drain timeout and were aborted");
        }
        if health_failure.is_none() {
            stop_health_task(&mut health_task).await;
        }

        self.deregister_runtimes(registered_control.runtime_ids())
            .await;
        if let Some(error) = health_failure {
            return Err(error);
        }
        if let Some(error) = owner_failure {
            return Err(error);
        }
        let successor = host.scheduled_restart_binary();
        Ok(DaemonStackExit {
            successor_binary: (!successor.is_empty()).then(|| PathBuf::from(successor)),
        })
    }

    async fn deregister_runtimes(&self, runtime_ids: Vec<String>) {
        if runtime_ids.is_empty() {
            return;
        }
        // The daemon root is already cancelled; shutdown delivery requires a
        // fresh lifetime exactly like Go's context.Background()+timeout.
        let deregister_ctx = Ctx::new();
        let result = tokio::time::timeout(
            DEREGISTER_TIMEOUT,
            self.client
                .deregister(&deregister_ctx, &runtime_ids, HashMap::new()),
        )
        .await;
        match result {
            Ok(Ok(())) => tracing::info!(count = runtime_ids.len(), "deregistered runtimes"),
            Ok(Err(error)) => tracing::warn!(%error, "failed to deregister runtimes"),
            Err(_) => tracing::warn!("runtime deregistration timed out"),
        }
    }
}

struct HealthState<S: ProductionRuntimeServices> {
    config: Arc<Config>,
    services: Arc<S>,
    host: Arc<DaemonCoreHost<S>>,
    repo_cache: Arc<Cache>,
    checkout_registry: Arc<RepoCheckoutRegistry>,
    ready: Arc<AtomicBool>,
    root_ctx: Ctx,
    started_at: SystemTime,
    clock: Arc<dyn BootstrapClock>,
}

impl<S: ProductionRuntimeServices> Clone for HealthState<S> {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            services: Arc::clone(&self.services),
            host: Arc::clone(&self.host),
            repo_cache: Arc::clone(&self.repo_cache),
            checkout_registry: Arc::clone(&self.checkout_registry),
            ready: Arc::clone(&self.ready),
            root_ctx: self.root_ctx.clone(),
            started_at: self.started_at,
            clock: Arc::clone(&self.clock),
        }
    }
}

fn spawn_health_server<S: ProductionRuntimeServices>(
    listener: TcpListener,
    state: Arc<HealthState<S>>,
    ctx: Ctx,
) -> JoinHandle<anyhow::Result<()>> {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/health", get(health_handler::<S>))
            .route("/shutdown", post(shutdown_handler::<S>))
            .route("/repo/checkout", post(repo_checkout_handler::<S>))
            .with_state(state);
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { ctx.cancelled().await })
            .await
            .map_err(anyhow::Error::from)
    })
}

async fn stop_health_task(task: &mut JoinHandle<anyhow::Result<()>>) {
    match tokio::time::timeout(OWNED_DRAIN_TIMEOUT, &mut *task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::warn!(%error, "daemon health server failed"),
        Ok(Err(error)) => tracing::warn!(%error, "daemon health task join failed"),
        Err(_) => {
            task.abort();
            let _ = task.await;
            tracing::warn!("daemon health server exceeded drain timeout");
        }
    }
}

async fn health_handler<S: ProductionRuntimeServices>(
    State(state): State<Arc<HealthState<S>>>,
) -> Json<HealthResponse> {
    let mut response = state.services.health_snapshot();
    response.status = if state.ready.load(Ordering::Acquire) {
        "running".to_string()
    } else {
        "starting".to_string()
    };
    response.pid = std::process::id() as i32;
    response.os = std::env::consts::OS.to_string();
    response.uptime = format!(
        "{}s",
        state
            .clock
            .now()
            .duration_since(state.started_at)
            .unwrap_or_default()
            .as_secs()
    );
    response.profile = state.config.profile.clone();
    response.daemon_id = state.config.daemon_id.clone();
    response.device_name = state.config.device_name.clone();
    response.server_url = state.config.server_base_url.clone();
    response.cli_version = state.config.cli_version.clone();
    response.launched_by = state.config.launched_by.clone();
    response.active_task_count = state.host.activity().active_tasks() as i64;
    let cache_activity = state.repo_cache.activity();
    response.repo_maintenance_active = cache_activity.maintenance_active as i64;
    response.repo_checkout_waiters = cache_activity.foreground_waiters as i64;
    response.reload_pending_reason = state.host.reload_pending_reason().unwrap_or_default();
    Json(response)
}

async fn shutdown_handler<S: ProductionRuntimeServices>(
    State(state): State<Arc<HealthState<S>>>,
) -> Json<Value> {
    let ctx = state.root_ctx.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        ctx.cancel_with(CancelCause::Shutdown);
    });
    Json(json!({"status":"shutting down"}))
}

async fn repo_checkout_handler<S: ProductionRuntimeServices>(
    State(state): State<Arc<HealthState<S>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, (StatusCode, HeaderMap, String)> {
    let unauthorized = || {
        response_error(
            StatusCode::UNAUTHORIZED,
            "repo checkout requires an active task credential",
        )
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(unauthorized)?;
    let active = state
        .checkout_registry
        .resolve(authorization)
        .ok_or_else(unauthorized)?;
    let mut request: RepoCheckoutRequest = serde_json::from_slice(&body).map_err(|error| {
        response_owned_error(
            StatusCode::BAD_REQUEST,
            format!("invalid request body: {error}"),
        )
    })?;
    request.url = request.url.trim().to_string();
    if request.url.is_empty() {
        return Err(response_error(StatusCode::BAD_REQUEST, "url is required"));
    }
    if request.workspace_id.is_empty() {
        return Err(response_error(
            StatusCode::BAD_REQUEST,
            "workspace_id is required",
        ));
    }
    if request.workdir.is_empty() {
        return Err(response_error(
            StatusCode::BAD_REQUEST,
            "workdir is required",
        ));
    }
    if !request.checkout_mode.is_empty() && request.checkout_mode != CHECKOUT_MODE_ISOLATED {
        return Err(response_error(
            StatusCode::BAD_REQUEST,
            "invalid checkout_mode",
        ));
    }
    if request.workspace_id != active.workspace_id || request.task_id != active.task_id {
        return Err(response_error(
            StatusCode::FORBIDDEN,
            "repo checkout task context does not match the active task",
        ));
    }
    let authorized =
        authorize_repo_checkout_workdir(&active.work_dir, &request.workdir).map_err(|_| {
            response_error(
                StatusCode::FORBIDDEN,
                "repo checkout workdir is not owned by the active task",
            )
        })?;
    request.workspace_id = active.workspace_id.clone();
    request.task_id = active.task_id.clone();
    request.agent_name = active.agent_name.clone();
    request.workdir = authorized.to_string_lossy().into_owned();

    let retry_aware = request.retry_busy;
    let request_lifetime = RequestLifetime(state.root_ctx.child());
    let result = state
        .services
        .repo_checkout(request_lifetime.0.clone(), active, request)
        .await
        .map_err(|failure| {
            let status = StatusCode::from_u16(failure.status_code)
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut headers = HeaderMap::new();
            if failure.retryable_busy && retry_aware {
                headers.insert("X-Cordy-Retryable", HeaderValue::from_static("repo-busy"));
                headers.insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
            }
            (status, headers, format!("{}\n", failure.message))
        })?;
    Ok(Json(result))
}

struct RequestLifetime(Ctx);

impl Drop for RequestLifetime {
    fn drop(&mut self) {
        self.0.cancel_with(CancelCause::Cancelled);
    }
}

fn response_error(status: StatusCode, message: &'static str) -> (StatusCode, HeaderMap, String) {
    response_owned_error(status, message.to_string())
}

fn response_owned_error(status: StatusCode, message: String) -> (StatusCode, HeaderMap, String) {
    (status, HeaderMap::new(), format!("{message}\n"))
}
