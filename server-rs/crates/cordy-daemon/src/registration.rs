//! Ordered workspace/runtime registration orchestration.
//!
//! Provider integration supplies one machine-level probe round and derives
//! each workspace's custom-profile payload. The daemon owns API ordering,
//! authoritative state application, dropped-row cleanup, orphan recovery, and
//! membership removal.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::client::{Client, WorkspaceInfo};
use crate::config::Config;
use crate::repo_state::DaemonRepoState;
use crate::repocache::{Ctx, RepoInfo};
use crate::runtime_registry::RuntimeRegistry;

const WORKSPACE_SYNC_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationPayload {
    /// Exact `/api/daemon/register` runtime entries. The provider integration
    /// owns version probing and custom-profile launch resolution.
    pub runtimes: Vec<BTreeMap<String, String>>,
    pub failed_profiles: Vec<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinRefreshReason {
    Discovery,
    Version,
}

/// Best-effort cache warmup request. Correctness does not depend on queue
/// delivery: the authenticated checkout handler synchronizes a miss on demand.
pub(crate) struct RepoWarmupRequest {
    pub workspace_id: String,
    pub repos: Vec<RepoInfo>,
}

pub(crate) fn enqueue_repo_warmup(
    tx: &mpsc::Sender<RepoWarmupRequest>,
    workspace_id: impl Into<String>,
    repos: Vec<RepoInfo>,
) {
    if repos.is_empty() {
        return;
    }
    let request = RepoWarmupRequest {
        workspace_id: workspace_id.into(),
        repos,
    };
    match tx.try_send(request) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(request)) => tracing::debug!(
            workspace_id = %request.workspace_id,
            repos = request.repos.len(),
            "repo warmup queue full; checkout will synchronize on demand"
        ),
        Err(mpsc::error::TrySendError::Closed(request)) => tracing::warn!(
            workspace_id = %request.workspace_id,
            repos = request.repos.len(),
            "repo warmup owner stopped"
        ),
    }
}

#[async_trait::async_trait]
pub trait RuntimeRegistrationRound: Send + Sync + 'static {
    /// A successful empty payload is authoritative for refresh rounds: the
    /// workspace currently has no runnable providers. Transient probe/fetch
    /// failures must be returned as `Err` rather than disguised as empty.
    async fn payload_for_workspace(
        &self,
        ctx: Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<RegistrationPayload>;
}

#[async_trait::async_trait]
pub trait RuntimeRegistrationSource: Send + Sync + 'static {
    /// Starts one machine-level probe round. The returned object is reused for
    /// every workspace in this sync so N workspaces never cause N×M CLI probes.
    async fn begin_round(&self, ctx: Ctx) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>>;

    /// Probes built-in providers for the requested cadence. `None` means a
    /// real probe found no registration change; `Some` carries one shared
    /// built-in-only round for every tracked workspace.
    async fn begin_builtin_refresh(
        &self,
        ctx: Ctx,
        reason: BuiltinRefreshReason,
    ) -> anyhow::Result<Option<Arc<dyn RuntimeRegistrationRound>>>;

    /// Releases provider-owned launch state when workspace membership is
    /// removed. A later re-add must not revive stale custom profile commands
    /// if its first profile fetch fails.
    fn workspace_removed(&self, workspace_id: &str);
}

pub struct RuntimeRegistrationService<S: RuntimeRegistrationSource> {
    config: Arc<Config>,
    client: Arc<Client>,
    repo_state: Arc<DaemonRepoState>,
    repo_warmups: mpsc::Sender<RepoWarmupRequest>,
    source: Arc<S>,
    serial: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl<S: RuntimeRegistrationSource> RuntimeRegistrationService<S> {
    pub fn new(
        config: Arc<Config>,
        client: Arc<Client>,
        repo_state: Arc<DaemonRepoState>,
        repo_warmups: mpsc::Sender<RepoWarmupRequest>,
        source: Arc<S>,
    ) -> Self {
        Self {
            config,
            client,
            repo_state,
            repo_warmups,
            source,
            serial: Mutex::new(HashMap::new()),
        }
    }

    /// One workspace membership/reconciliation round. Startup calls this with
    /// `reconcile_profiles=false`; WebSocket reconnect calls it with `true` so
    /// profile changes missed while disconnected are folded into registration.
    pub async fn sync_once(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        reconcile_profiles: bool,
    ) -> anyhow::Result<()> {
        let workspaces =
            tokio::time::timeout(WORKSPACE_SYNC_TIMEOUT, self.client.list_workspaces(&ctx))
                .await
                .map_err(|_| anyhow::anyhow!("list workspaces timed out"))??;
        let api_ids: BTreeSet<String> = workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect();
        let tracked: BTreeSet<String> = registry.workspace_ids().into_iter().collect();
        let needs_registration: Vec<WorkspaceInfo> = workspaces
            .into_iter()
            .filter(|workspace| {
                reconcile_profiles
                    || !tracked.contains(&workspace.id)
                    || registry.workspace_needs_runtime_recovery(&workspace.id)
            })
            .collect();

        let mut registered = 0usize;
        if !needs_registration.is_empty() {
            let round = self.source.begin_round(ctx.child()).await?;
            for workspace in &needs_registration {
                let recover_orphans = !tracked.contains(&workspace.id)
                    || registry.workspace_needs_runtime_recovery(&workspace.id);
                match self
                    .register_workspace(
                        ctx.child(),
                        registry,
                        workspace,
                        Arc::clone(&round),
                        recover_orphans,
                        reconcile_profiles && tracked.contains(&workspace.id),
                    )
                    .await
                {
                    Ok(()) => registered += 1,
                    Err(error) => tracing::warn!(
                        workspace_id = %workspace.id,
                        workspace_name = %workspace.name,
                        %error,
                        "failed to register workspace runtimes"
                    ),
                }
            }
        }

        for workspace_id in tracked.difference(&api_ids) {
            registry.remove_workspace(workspace_id);
            self.source.workspace_removed(workspace_id);
            self.repo_state.remove_workspace(workspace_id);
            tracing::info!(%workspace_id, "stopped watching workspace");
        }

        if registry.runtime_ids().is_empty() && registered == 0 && !api_ids.is_empty() {
            anyhow::bail!(
                "failed to register runtimes for any of the {} workspace(s)",
                api_ids.len()
            );
        }
        Ok(())
    }

    /// Runtime-gone recovery prunes the stale identity before re-registering
    /// its workspace with a fresh probe round. Orphan recovery is required for
    /// the new response because the previous process/runtime row disappeared.
    pub async fn recover_runtime_gone(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        runtime_id: &str,
    ) -> anyhow::Result<()> {
        let workspace_id = registry
            .remove_runtime(runtime_id)
            .ok_or_else(|| anyhow::anyhow!("runtime {runtime_id} is not registered"))?;
        let workspace = registry
            .workspace(&workspace_id)
            .ok_or_else(|| anyhow::anyhow!("workspace {workspace_id} is no longer tracked"))?;
        let round = self.source.begin_round(ctx.child()).await?;
        self.register_workspace(
            ctx,
            registry,
            &WorkspaceInfo {
                id: workspace.id,
                name: workspace.name,
            },
            round,
            true,
            false,
        )
        .await
    }

    /// Re-registers one tracked workspace for an on-demand runtime-profile
    /// notification. This path deliberately does not recover orphans: existing
    /// tasks on surviving runtimes remain valid, while dropped rows are taken
    /// offline by the ordered registration cleanup.
    pub async fn refresh_workspace(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        workspace_id: &str,
    ) -> anyhow::Result<()> {
        let workspace = registry
            .workspace(workspace_id)
            .ok_or_else(|| anyhow::anyhow!("workspace {workspace_id} is not tracked"))?;
        let round = self.source.begin_round(ctx.child()).await?;
        self.register_workspace(
            ctx,
            registry,
            &WorkspaceInfo {
                id: workspace.id,
                name: workspace.name,
            },
            round,
            false,
            true,
        )
        .await
    }

    pub async fn refresh_builtins_once(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        reason: BuiltinRefreshReason,
    ) -> anyhow::Result<()> {
        let Some(round) = self
            .source
            .begin_builtin_refresh(ctx.child(), reason)
            .await?
        else {
            return Ok(());
        };
        for workspace_id in registry.workspace_ids() {
            let Some(workspace) = registry.workspace(&workspace_id) else {
                continue;
            };
            if let Err(error) = self
                .register_builtin_workspace(ctx.child(), registry, &workspace, Arc::clone(&round))
                .await
            {
                tracing::warn!(%workspace_id, %error, "built-in runtime refresh failed");
            }
        }
        Ok(())
    }

    async fn register_workspace(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        workspace: &WorkspaceInfo,
        round: Arc<dyn RuntimeRegistrationRound>,
        recover_orphans: bool,
        allow_empty_refresh: bool,
    ) -> anyhow::Result<()> {
        let serial = self.workspace_lock(&workspace.id);
        let _guard = serial.lock().await;
        let payload = round
            .payload_for_workspace(ctx.child(), &workspace.id)
            .await?;
        if payload.runtimes.is_empty() && payload.failed_profiles.is_empty() {
            anyhow::ensure!(
                allow_empty_refresh,
                "no runtimes to register for workspace {}",
                workspace.id
            );
            // The register endpoint rejects an empty request, but a successful
            // profile refresh returning no providers is still authoritative.
            // Publish zero locally first so WebSocket/heartbeat/claim stop
            // using stale IDs, then take those rows offline server-side while
            // the workspace registration serial remains held.
            let delta = registry.apply_registration(
                workspace.id.clone(),
                workspace.name.clone(),
                Vec::new(),
            )?;
            self.deregister_dropped(&ctx, &delta.dropped).await?;
            tracing::info!(
                workspace_id = %workspace.id,
                dropped = delta.dropped.len(),
                "workspace runtime profiles converged to zero"
            );
            return Ok(());
        }
        let response = self
            .client
            .register(
                &ctx,
                json!({
                    "workspace_id": &workspace.id,
                    "daemon_id": &self.config.daemon_id,
                    "legacy_daemon_ids": &self.config.legacy_daemon_ids,
                    "device_name": &self.config.device_name,
                    "cli_version": &self.config.cli_version,
                    "launched_by": &self.config.launched_by,
                    "runtimes": &payload.runtimes,
                    "failed_profiles": &payload.failed_profiles,
                }),
            )
            .await?;
        anyhow::ensure!(
            !response.runtimes.is_empty() || !payload.failed_profiles.is_empty(),
            "register runtimes returned an empty response for workspace {}",
            workspace.id
        );
        let runtime_ids: Vec<String> = response
            .runtimes
            .iter()
            .map(|runtime| runtime.id.clone())
            .collect();
        let repos = response.repos;
        let settings = response.settings;
        let delta = registry.apply_registration(
            workspace.id.clone(),
            workspace.name.clone(),
            response.runtimes,
        )?;
        self.repo_state
            .replace_workspace(&workspace.id, &repos, settings);
        self.deregister_dropped(&ctx, &delta.dropped).await?;
        if recover_orphans {
            for runtime_id in runtime_ids {
                if let Err(error) = self.client.recover_orphans(&ctx, &runtime_id).await {
                    tracing::warn!(%runtime_id, %error, "recover-orphans failed");
                }
            }
        }
        self.enqueue_workspace_repos(&workspace.id, repos);
        Ok(())
    }

    async fn register_builtin_workspace(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        workspace: &crate::runtime_registry::WorkspaceRuntimeState,
        round: Arc<dyn RuntimeRegistrationRound>,
    ) -> anyhow::Result<()> {
        let serial = self.workspace_lock(&workspace.id);
        let _guard = serial.lock().await;
        let payload = round
            .payload_for_workspace(ctx.child(), &workspace.id)
            .await?;
        anyhow::ensure!(
            payload.failed_profiles.is_empty(),
            "built-in refresh returned profile failures for workspace {}",
            workspace.id
        );
        if payload.runtimes.is_empty() {
            // `Some(round)` means the provider completed an authoritative
            // changed probe. Applying an empty built-in set removes the last
            // vanished executable while preserving custom-profile runtimes.
            let delta =
                registry.apply_builtin_registration(&workspace.id, &workspace.name, Vec::new())?;
            self.deregister_dropped(&ctx, &delta.dropped).await?;
            return Ok(());
        }
        let response = self
            .client
            .register(
                &ctx,
                json!({
                    "workspace_id": &workspace.id,
                    "daemon_id": &self.config.daemon_id,
                    "legacy_daemon_ids": &self.config.legacy_daemon_ids,
                    "device_name": &self.config.device_name,
                    "cli_version": &self.config.cli_version,
                    "launched_by": &self.config.launched_by,
                    "runtimes": &payload.runtimes,
                    "failed_profiles": Vec::<BTreeMap<String, String>>::new(),
                }),
            )
            .await?;
        anyhow::ensure!(
            !response.runtimes.is_empty(),
            "built-in register returned an empty response for workspace {}",
            workspace.id
        );
        let delta = registry.apply_builtin_registration(
            &workspace.id,
            &workspace.name,
            response.runtimes,
        )?;
        self.deregister_dropped(&ctx, &delta.dropped).await?;
        Ok(())
    }

    async fn deregister_dropped(&self, ctx: &Ctx, runtime_ids: &[String]) -> anyhow::Result<()> {
        if runtime_ids.is_empty() {
            return Ok(());
        }
        self.client
            .deregister(ctx, runtime_ids, HashMap::new())
            .await
    }

    fn workspace_lock(&self, workspace_id: &str) -> Arc<AsyncMutex<()>> {
        let mut serial = self.serial.lock().unwrap();
        Arc::clone(
            serial
                .entry(workspace_id.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
        )
    }

    fn enqueue_workspace_repos(&self, workspace_id: &str, repos: Vec<crate::types::RepoData>) {
        if repos.is_empty() {
            return;
        }
        let repos: Vec<RepoInfo> = repos
            .into_iter()
            .filter(|repo| !repo.url.is_empty())
            .map(|repo| RepoInfo { url: repo.url })
            .collect();
        if repos.is_empty() {
            return;
        }
        enqueue_repo_warmup(&self.repo_warmups, workspace_id.to_string(), repos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn repo_warmup_queue_is_bounded_and_keeps_existing_work() {
        let (tx, mut rx) = mpsc::channel(1);
        enqueue_repo_warmup(
            &tx,
            "workspace-1",
            vec![RepoInfo {
                url: "https://example.test/first.git".into(),
            }],
        );
        enqueue_repo_warmup(
            &tx,
            "workspace-2",
            vec![RepoInfo {
                url: "https://example.test/second.git".into(),
            }],
        );

        let queued = rx.try_recv().unwrap();
        assert_eq!(queued.workspace_id, "workspace-1");
        assert_eq!(queued.repos[0].url, "https://example.test/first.git");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn registration_payload_preserves_provider_owned_wire_fields() {
        let payload = RegistrationPayload {
            runtimes: vec![BTreeMap::from([
                ("type".to_string(), "codex".to_string()),
                ("version".to_string(), "1.2.3".to_string()),
                ("profile_id".to_string(), "profile-1".to_string()),
            ])],
            failed_profiles: vec![BTreeMap::from([(
                "profile_id".to_string(),
                "profile-2".to_string(),
            )])],
        };

        let wire = json!({
            "runtimes": payload.runtimes,
            "failed_profiles": payload.failed_profiles,
        });
        assert_eq!(wire["runtimes"][0]["type"], Value::String("codex".into()));
        assert_eq!(
            wire["failed_profiles"][0]["profile_id"],
            Value::String("profile-2".into())
        );
    }
}
