//! Concrete daemon-core host composition.
//!
//! This owner joins transport lifecycle, task orchestration, auto-update, and
//! GC around one [`DaemonActivity`]. Provider execution remains a required
//! service while binary replacement is owned by the production
//! [`UpdateExecutor`]; no default/no-op behavior is available.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::json;

use cordy_protocol::{
    DaemonHeartbeatAckPayload, DaemonHeartbeatPendingUpdate, RuntimeProfilesChangedPayload,
};

use crate::activity::DaemonActivity;
use crate::auto_update::{AutoUpdateHost, AutoUpdateSettings};
use crate::client::{request_status_code, Client};
use crate::config::Config;
use crate::control_lifecycle::DaemonControlLifecycle;
use crate::gc::{
    GcConfig, GcHost, IssueGCCheckResult, IssueGCCheckStatus, RequestError as GcRequestError,
};
use crate::repocache::{Cache, CancelCause, Ctx};
use crate::runtime_registry::RuntimeRegistry;
use crate::task_execution::{DaemonTaskExecutionHost, TaskRunOutcome};
use crate::types::{RuntimeExecutionTarget, Task};
use crate::update_executor::UpdateExecutor;

const UPDATE_REPORT_BACKOFFS: &[Duration] = &[
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Required platform/provider services composed by [`DaemonCoreHost`]. The
/// binary bootstrap supplies one real implementation; tests must supply every
/// operation explicitly.
#[async_trait::async_trait]
pub trait DaemonCoreServices: Send + Sync + 'static {
    async fn handle_runtime_gone(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        runtime_id: String,
    );
    async fn refresh_workspace_runtime_profiles(
        &self,
        ctx: Ctx,
        registry: Arc<RuntimeRegistry>,
        activity: Arc<DaemonActivity>,
        payload: RuntimeProfilesChangedPayload,
    );
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
    ) -> TaskRunOutcome;

    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool;
}

pub(crate) struct DaemonCoreHost<S: DaemonCoreServices> {
    config: Arc<Config>,
    client: Arc<Client>,
    repo_cache: Arc<Cache>,
    services: Arc<S>,
    registry: Arc<RuntimeRegistry>,
    update_executor: Arc<UpdateExecutor>,
    activity: Arc<DaemonActivity>,
    root_ctx: Ctx,
    auto_update: AutoUpdateSettings,
    gc: GcConfig,
    updating: AtomicBool,
    restart_binary: Mutex<String>,
    reload_pending: Mutex<Option<String>>,
}

/// Cohesive production dependencies required to own the daemon control plane.
/// Keeping this typed prevents call sites from accidentally swapping the many
/// shared owners that must all describe the same daemon instance.
pub(crate) struct DaemonCoreDependencies<S: DaemonCoreServices> {
    pub(crate) config: Arc<Config>,
    pub(crate) client: Arc<Client>,
    pub(crate) repo_cache: Arc<Cache>,
    pub(crate) services: Arc<S>,
    pub(crate) registry: Arc<RuntimeRegistry>,
    pub(crate) update_executor: Arc<UpdateExecutor>,
    pub(crate) activity: Arc<DaemonActivity>,
    pub(crate) root_ctx: Ctx,
}

impl<S: DaemonCoreServices> DaemonCoreHost<S> {
    pub(crate) fn new(dependencies: DaemonCoreDependencies<S>) -> Self {
        let DaemonCoreDependencies {
            config,
            client,
            repo_cache,
            services,
            registry,
            update_executor,
            activity,
            root_ctx,
        } = dependencies;
        let auto_update = AutoUpdateSettings {
            launched_by: config.launched_by.clone(),
            cli_version: config.cli_version.clone(),
            auto_update_enabled: config.auto_update_enabled,
            auto_reload_enabled: config.auto_reload_enabled,
            auto_update_check_interval: config.auto_update_check_interval,
        };
        let gc = GcConfig {
            profile: config.profile.clone(),
            workspaces_root: PathBuf::from(&config.workspaces_root),
            gc_enabled: config.gc_enabled,
            gc_interval: config.gc_interval,
            gc_ttl: config.gc_ttl,
            gc_completed_task_ttl: config.gc_completed_task_ttl,
            gc_orphan_ttl: config.gc_orphan_ttl,
            gc_artifact_ttl: config.gc_artifact_ttl,
            gc_codex_session_ttl: config.gc_codex_session_ttl,
            gc_hermes_memory_ttl: config.gc_hermes_memory_ttl,
            gc_hermes_session_ttl: config.gc_hermes_session_ttl,
            gc_repo_ttl: config.gc_repo_ttl,
            gc_repo_maintenance_enabled: config.gc_repo_maintenance_enabled,
            gc_artifact_patterns: config.gc_artifact_patterns.clone(),
        };
        Self {
            config,
            client,
            repo_cache,
            services,
            registry,
            update_executor,
            activity,
            root_ctx,
            auto_update,
            gc,
            updating: AtomicBool::new(false),
            restart_binary: Mutex::new(String::new()),
            reload_pending: Mutex::new(None),
        }
    }

    pub(crate) fn activity(&self) -> &Arc<DaemonActivity> {
        &self.activity
    }

    pub(crate) fn scheduled_restart_binary(&self) -> String {
        self.restart_binary.lock().unwrap().clone()
    }

    pub(crate) fn reload_pending_reason(&self) -> Option<String> {
        self.reload_pending.lock().unwrap().clone()
    }

    fn trigger_restart_inner(&self) -> bool {
        if !self.scheduled_restart_binary().is_empty() {
            return true;
        }
        let target = match self
            .update_executor
            .restart_target_binary()
            .as_os_str()
            .to_str()
        {
            Some(target) if !target.is_empty() => target.to_string(),
            _ => {
                tracing::error!("daemon restart target is not valid UTF-8");
                return false;
            }
        };
        *self.restart_binary.lock().unwrap() = target;
        self.root_ctx.cancel_with(CancelCause::Shutdown);
        true
    }

    async fn report_update_result_with_retry(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        update_id: &str,
        payload: serde_json::Value,
    ) {
        for (attempt, wait) in UPDATE_REPORT_BACKOFFS.iter().copied().enumerate() {
            if !wait.is_zero() {
                tokio::select! {
                    () = ctx.cancelled() => return,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            match self
                .client
                .report_update_result(ctx, runtime_id, update_id, payload.clone())
                .await
            {
                Ok(()) => return,
                Err(err) => {
                    let permanent = request_status_code(&err)
                        .is_some_and(|status| (400..500).contains(&status));
                    if permanent || attempt + 1 == UPDATE_REPORT_BACKOFFS.len() {
                        tracing::error!(%runtime_id, %update_id, error = %err, "CLI update report failed");
                        return;
                    }
                    tracing::warn!(%runtime_id, %update_id, error = %err, "CLI update report failed; retrying");
                }
            }
        }
    }

    async fn handle_server_update(
        &self,
        ctx: Ctx,
        runtime_id: String,
        update: DaemonHeartbeatPendingUpdate,
    ) {
        if self.config.launched_by == "desktop" {
            self.report_update_result_with_retry(
                &ctx,
                &runtime_id,
                &update.id,
                json!({
                    "status": "failed",
                    "error": "CLI is managed by Cordy Desktop — update the Desktop app to upgrade the CLI",
                }),
            )
            .await;
            return;
        }
        if self
            .updating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.report_update_result_with_retry(
                &ctx,
                &runtime_id,
                &update.id,
                json!({"status":"failed","error":"another runtime update is already in progress on this machine"}),
            )
            .await;
            return;
        }
        if !self.activity.pause_claims_when_idle(&ctx).await {
            self.updating.store(false, Ordering::Release);
            self.report_update_result_with_retry(
                &ctx,
                &runtime_id,
                &update.id,
                json!({"status":"failed","error":"runtime update deferred because agent work is starting or still active; retry when the machine is idle"}),
            )
            .await;
            return;
        }

        self.report_update_result_with_retry(
            &ctx,
            &runtime_id,
            &update.id,
            json!({"status":"running"}),
        )
        .await;
        match self.update_executor.update(&update.target_version).await {
            Ok(output) => {
                tracing::info!(%output, target = %update.target_version, "CLI update completed successfully");
                self.report_update_result_with_retry(
                    &ctx,
                    &runtime_id,
                    &update.id,
                    json!({"status":"completed","output":format!("Updated to {}", update.target_version)}),
                )
                .await;
                if self.trigger_restart_inner() {
                    return;
                }
                tracing::error!("CLI update completed but restart could not be scheduled");
            }
            Err(err) => {
                tracing::error!(error = %err, target = %update.target_version, "CLI update failed");
                self.report_update_result_with_retry(
                    &ctx,
                    &runtime_id,
                    &update.id,
                    json!({"status":"failed","error":err.to_string()}),
                )
                .await;
            }
        }
        self.activity.release_claim_barrier();
        self.updating.store(false, Ordering::Release);
    }
}

#[async_trait::async_trait]
impl<S: DaemonCoreServices> DaemonControlLifecycle for DaemonCoreHost<S> {
    async fn handle_runtime_gone(&self, ctx: Ctx, runtime_id: String) {
        self.services
            .handle_runtime_gone(ctx, Arc::clone(&self.registry), runtime_id)
            .await;
    }

    async fn refresh_workspace_runtime_profiles(
        &self,
        ctx: Ctx,
        payload: RuntimeProfilesChangedPayload,
    ) {
        self.services
            .refresh_workspace_runtime_profiles(
                ctx,
                Arc::clone(&self.registry),
                Arc::clone(&self.activity),
                payload,
            )
            .await;
    }

    async fn handle_heartbeat_actions(
        &self,
        ctx: Ctx,
        runtime_id: String,
        mut ack: DaemonHeartbeatAckPayload,
    ) {
        let update = ack.pending_update.take();
        let other = self.services.handle_non_update_heartbeat_actions(
            ctx.child(),
            Arc::clone(&self.registry),
            runtime_id.clone(),
            ack,
        );
        let update = async {
            if let Some(update) = update {
                self.handle_server_update(ctx, runtime_id, update).await;
            }
        };
        tokio::join!(other, update);
    }
}

#[async_trait::async_trait]
impl<S: DaemonCoreServices> DaemonTaskExecutionHost for DaemonCoreHost<S> {
    fn execution_target_for_runtime(&self, runtime_id: &str) -> Option<RuntimeExecutionTarget> {
        self.registry.execution_target_for_runtime(runtime_id)
    }

    async fn cancel_repository_maintenance(&self) {
        self.repo_cache.cancel_maintenance().await;
    }

    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        target: RuntimeExecutionTarget,
        slot: usize,
    ) -> TaskRunOutcome {
        self.services
            .run_task(ctx, task, target, slot, Arc::clone(&self.activity))
            .await
    }
}

impl<S: DaemonCoreServices> AutoUpdateHost for DaemonCoreHost<S> {
    fn settings(&self) -> &AutoUpdateSettings {
        &self.auto_update
    }

    fn updating_load(&self) -> bool {
        self.updating.load(Ordering::Acquire)
    }

    fn updating_cas_acquire(&self) -> bool {
        self.updating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn updating_store_false(&self) {
        self.updating.store(false, Ordering::Release);
    }

    fn activity(&self) -> &Arc<DaemonActivity> {
        &self.activity
    }

    fn restart_binary(&self) -> String {
        self.scheduled_restart_binary()
    }

    fn run_update<'a>(&'a self, target_version: &'a str) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(self.update_executor.update(target_version))
    }

    fn trigger_restart(&self) -> bool {
        self.trigger_restart_inner()
    }

    fn restart_target_binary(&self) -> anyhow::Result<String> {
        self.update_executor
            .restart_target_binary()
            .as_os_str()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("daemon restart target is not valid UTF-8"))
    }

    fn set_reload_pending(&self, reason: Option<String>) {
        *self.reload_pending.lock().unwrap() = reason;
    }
}

impl<S: DaemonCoreServices> GcHost for DaemonCoreHost<S> {
    fn config(&self) -> &GcConfig {
        &self.gc
    }

    async fn get_issue_gc_check(
        &self,
        ctx: &Ctx,
        issue_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.client
            .get_issue_gc_check(ctx, issue_id)
            .await
            .map(|status| IssueGCCheckStatus {
                status: status.status,
                updated_at: status.updated_at,
            })
            .map_err(map_gc_error)
    }

    async fn get_issue_gc_checks(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, IssueGCCheckResult>> {
        self.client
            .get_issue_gc_checks(ctx, workspace_id, issue_ids)
            .await
            .map(|results| {
                results
                    .into_iter()
                    .map(|(id, result)| {
                        (
                            id,
                            IssueGCCheckResult {
                                found: result.found,
                                status: result.status,
                                updated_at: result.updated_at,
                                err: result.err,
                            },
                        )
                    })
                    .collect()
            })
            .map_err(map_gc_error)
    }

    async fn get_chat_session_gc_check(
        &self,
        ctx: &Ctx,
        chat_session_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.client
            .get_chat_session_gc_check(ctx, chat_session_id)
            .await
            .map(|status| IssueGCCheckStatus {
                status: status.status,
                updated_at: status.updated_at,
            })
            .map_err(map_gc_error)
    }

    async fn get_autopilot_run_gc_check(
        &self,
        ctx: &Ctx,
        autopilot_run_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.client
            .get_autopilot_run_gc_check(ctx, autopilot_run_id)
            .await
            .map(|status| IssueGCCheckStatus {
                status: status.status,
                updated_at: status.completed_at,
            })
            .map_err(map_gc_error)
    }

    async fn get_task_gc_check(
        &self,
        ctx: &Ctx,
        task_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.client
            .get_task_gc_check(ctx, task_id)
            .await
            .map(|status| IssueGCCheckStatus {
                status: status.status,
                updated_at: status.completed_at,
            })
            .map_err(map_gc_error)
    }

    fn activity(&self) -> &Arc<DaemonActivity> {
        &self.activity
    }

    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool {
        self.services.repo_bare_path_is_live(bare_path)
    }

    fn repo_cache_for_gc(&self) -> Option<&Cache> {
        Some(self.repo_cache.as_ref())
    }
}

fn map_gc_error(err: anyhow::Error) -> anyhow::Error {
    if request_status_code(&err) == Some(404) {
        anyhow::Error::new(GcRequestError { status_code: 404 })
    } else {
        err
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::runtime_set::RuntimeSet;
    use crate::types::TaskResult;

    #[derive(Default)]
    struct RecordingServices {
        heartbeat_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DaemonCoreServices for RecordingServices {
        async fn handle_runtime_gone(
            &self,
            _ctx: Ctx,
            _registry: Arc<RuntimeRegistry>,
            _runtime_id: String,
        ) {
        }

        async fn refresh_workspace_runtime_profiles(
            &self,
            _ctx: Ctx,
            _registry: Arc<RuntimeRegistry>,
            _activity: Arc<DaemonActivity>,
            _payload: RuntimeProfilesChangedPayload,
        ) {
        }

        async fn handle_non_update_heartbeat_actions(
            &self,
            _ctx: Ctx,
            _registry: Arc<RuntimeRegistry>,
            _runtime_id: String,
            _ack: DaemonHeartbeatAckPayload,
        ) {
            self.heartbeat_calls.fetch_add(1, Ordering::SeqCst);
        }

        async fn run_task(
            &self,
            _ctx: Ctx,
            _task: Task,
            _target: RuntimeExecutionTarget,
            _slot: usize,
            _activity: Arc<DaemonActivity>,
        ) -> TaskRunOutcome {
            TaskRunOutcome {
                result: TaskResult::default(),
                failure: None,
            }
        }

        fn repo_bare_path_is_live(&self, _bare_path: &Path) -> bool {
            false
        }
    }

    fn update_ack(id: &str, target: &str) -> DaemonHeartbeatAckPayload {
        serde_json::from_value(json!({
            "runtime_id": "runtime-1",
            "status": "ok",
            "pending_update": {"id": id, "target_version": target}
        }))
        .unwrap()
    }

    fn host(
        server_base_url: &str,
        launched_by: &str,
        root: &Path,
        services: Arc<RecordingServices>,
        activity: Arc<DaemonActivity>,
        root_ctx: Ctx,
    ) -> DaemonCoreHost<RecordingServices> {
        let config = Arc::new(Config {
            server_base_url: server_base_url.to_string(),
            launched_by: launched_by.to_string(),
            workspaces_root: root.to_string_lossy().into_owned(),
            ..Config::default()
        });
        DaemonCoreHost::new(DaemonCoreDependencies {
            client: Arc::new(Client::new(server_base_url)),
            repo_cache: Arc::new(Cache::new(root.join("repos"))),
            registry: Arc::new(RuntimeRegistry::new(Arc::new(RuntimeSet::new()))),
            update_executor: Arc::new(UpdateExecutor::direct_for_test(root.join("cordy"))),
            config,
            services,
            activity,
            root_ctx,
        })
    }

    #[tokio::test]
    async fn production_heartbeat_update_reports_and_restores_machine_barriers() {
        let reports = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
        let app = {
            let reports = Arc::clone(&reports);
            axum::Router::new().fallback(axum::routing::any(
                move |request: axum::extract::Request| {
                    let reports = Arc::clone(&reports);
                    async move {
                        let path = request.uri().path().to_string();
                        let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                            .await
                            .unwrap();
                        reports
                            .lock()
                            .unwrap()
                            .push((path, serde_json::from_slice(&body).unwrap()));
                        axum::http::StatusCode::NO_CONTENT
                    }
                },
            ))
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_stop = tokio_util::sync::CancellationToken::new();
        let stop = server_stop.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        let base_url = format!("http://{address}");
        let temp = tempfile::tempdir().unwrap();
        let services = Arc::new(RecordingServices::default());

        let desktop_activity = DaemonActivity::new();
        let desktop_root = Ctx::new();
        let desktop = host(
            &base_url,
            "desktop",
            temp.path(),
            Arc::clone(&services),
            Arc::clone(&desktop_activity),
            desktop_root.clone(),
        );
        desktop
            .handle_heartbeat_actions(
                desktop_root.clone(),
                "runtime-1".to_string(),
                update_ack("desktop", "v1.2.3"),
            )
            .await;
        assert!(!desktop_activity.claims_paused());
        assert!(desktop.scheduled_restart_binary().is_empty());

        let busy_activity = DaemonActivity::new();
        let claim = busy_activity.try_enter_claim().unwrap();
        let active_tasks = claim.handoff(vec![Vec::new()]).await;
        let busy_root = Ctx::new();
        let busy = host(
            &base_url,
            "",
            temp.path(),
            Arc::clone(&services),
            Arc::clone(&busy_activity),
            busy_root.clone(),
        );
        busy.handle_heartbeat_actions(
            busy_root.clone(),
            "runtime-1".to_string(),
            update_ack("busy", "v1.2.3"),
        )
        .await;
        assert!(!busy_activity.claims_paused());
        assert!(!busy.updating.load(Ordering::Acquire));
        drop(active_tasks);

        // Exercise the same machine-wide CAS used by the periodic updater,
        // rather than relying only on the activity barrier's busy response.
        let cas_activity = DaemonActivity::new();
        let cas_root = Ctx::new();
        let cas_busy = host(
            &base_url,
            "",
            temp.path(),
            Arc::clone(&services),
            Arc::clone(&cas_activity),
            cas_root.clone(),
        );
        cas_busy.updating.store(true, Ordering::Release);
        cas_busy
            .handle_heartbeat_actions(
                cas_root.clone(),
                "runtime-1".to_string(),
                update_ack("cas-busy", "v1.2.3"),
            )
            .await;
        assert!(cas_busy.updating.load(Ordering::Acquire));
        cas_busy.updating.store(false, Ordering::Release);
        assert!(!cas_activity.claims_paused());
        assert!(cas_root.err().is_none());

        let invalid_activity = DaemonActivity::new();
        let invalid_root = Ctx::new();
        let invalid = host(
            &base_url,
            "",
            temp.path(),
            Arc::clone(&services),
            Arc::clone(&invalid_activity),
            invalid_root.clone(),
        );
        invalid
            .handle_heartbeat_actions(
                invalid_root.clone(),
                "runtime-1".to_string(),
                update_ack("invalid", "not-a-version"),
            )
            .await;
        assert!(!invalid_activity.claims_paused());
        assert!(!invalid.updating.load(Ordering::Acquire));
        assert!(invalid_root.err().is_none());

        {
            let reports = reports.lock().unwrap();
            let payloads = |id: &str| {
                reports
                    .iter()
                    .filter(|(path, _)| path.ends_with(&format!("/update/{id}/result")))
                    .map(|(_, payload)| payload.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(payloads("desktop")[0]["status"], "failed");
            assert_eq!(payloads("busy")[0]["status"], "failed");
            assert_eq!(
                payloads("busy")[0]["error"],
                "runtime update deferred because agent work is starting or still active; retry when the machine is idle"
            );
            assert_eq!(
                payloads("cas-busy")[0]["error"],
                "another runtime update is already in progress on this machine"
            );
            let invalid_payloads = payloads("invalid");
            assert_eq!(invalid_payloads[0]["status"], "running");
            assert_eq!(invalid_payloads[1]["status"], "failed");
            assert!(invalid_payloads[1]["error"]
                .as_str()
                .unwrap()
                .contains("invalid-version"));
            assert_eq!(services.heartbeat_calls.load(Ordering::SeqCst), 4);
        }

        server_stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("update report fixture ignored shutdown")
            .unwrap();
    }
}
