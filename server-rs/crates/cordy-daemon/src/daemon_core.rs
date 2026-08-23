//! Concrete daemon-core host composition.
//!
//! This owner joins transport lifecycle, task orchestration, auto-update, and
//! GC around one [`DaemonActivity`]. Provider execution and platform-specific
//! binary replacement remain required services; no default/no-op behavior is
//! available in production.

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
use crate::task_execution::{DaemonTaskExecutionHost, TaskRunOutcome};
use crate::types::Task;

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
pub(crate) trait DaemonCoreServices: Send + Sync + 'static {
    async fn handle_runtime_gone(&self, ctx: Ctx, runtime_id: String);
    async fn refresh_workspace_runtime_profiles(
        &self,
        ctx: Ctx,
        payload: RuntimeProfilesChangedPayload,
    );
    async fn handle_non_update_heartbeat_actions(
        &self,
        ctx: Ctx,
        runtime_id: String,
        ack: DaemonHeartbeatAckPayload,
    );

    fn provider_for_runtime(&self, runtime_id: &str) -> Option<String>;
    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        provider: String,
        slot: usize,
        activity: Arc<DaemonActivity>,
    ) -> TaskRunOutcome;

    async fn run_update(&self, target_version: &str) -> anyhow::Result<String>;
    fn restart_target_binary(&self) -> anyhow::Result<String>;
    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool;
}

pub(crate) struct DaemonCoreHost<S: DaemonCoreServices> {
    config: Arc<Config>,
    client: Arc<Client>,
    repo_cache: Arc<Cache>,
    services: Arc<S>,
    activity: Arc<DaemonActivity>,
    root_ctx: Ctx,
    auto_update: AutoUpdateSettings,
    gc: GcConfig,
    updating: AtomicBool,
    restart_binary: Mutex<String>,
    reload_pending: Mutex<Option<String>>,
}

impl<S: DaemonCoreServices> DaemonCoreHost<S> {
    pub(crate) fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        repo_cache: Arc<Cache>,
        services: Arc<S>,
        activity: Arc<DaemonActivity>,
        root_ctx: Ctx,
    ) -> Self {
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
        let target = match self.services.restart_target_binary() {
            Ok(target) if !target.is_empty() => target,
            Ok(_) => return false,
            Err(err) => {
                tracing::error!(error = %err, "resolve daemon restart target failed");
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
        match self.services.run_update(&update.target_version).await {
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
        self.services.handle_runtime_gone(ctx, runtime_id).await;
    }

    async fn refresh_workspace_runtime_profiles(
        &self,
        ctx: Ctx,
        payload: RuntimeProfilesChangedPayload,
    ) {
        self.services
            .refresh_workspace_runtime_profiles(ctx, payload)
            .await;
    }

    async fn handle_heartbeat_actions(
        &self,
        ctx: Ctx,
        runtime_id: String,
        mut ack: DaemonHeartbeatAckPayload,
    ) {
        let update = ack.pending_update.take();
        let other =
            self.services
                .handle_non_update_heartbeat_actions(ctx.child(), runtime_id.clone(), ack);
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
    fn provider_for_runtime(&self, runtime_id: &str) -> Option<String> {
        self.services.provider_for_runtime(runtime_id)
    }

    async fn cancel_repository_maintenance(&self) {
        self.repo_cache.cancel_maintenance().await;
    }

    async fn run_task(
        &self,
        ctx: Ctx,
        task: Task,
        provider: String,
        slot: usize,
    ) -> TaskRunOutcome {
        self.services
            .run_task(ctx, task, provider, slot, Arc::clone(&self.activity))
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
        self.services.run_update(target_version)
    }

    fn trigger_restart(&self) -> bool {
        self.trigger_restart_inner()
    }

    fn restart_target_binary(&self) -> anyhow::Result<String> {
        self.services.restart_target_binary()
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
                                id: result.id,
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
