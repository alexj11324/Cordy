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

use crate::activity::{ClaimBarrierGuard, DaemonActivity};
use crate::agents_refresh::RuntimeVerdict;
use crate::client::{Client, WorkspaceInfo};
use crate::config::Config;
use crate::repo_state::DaemonRepoState;
use crate::repocache::{Ctx, RepoInfo};
use crate::runtime_registry::RuntimeRegistry;
use crate::types::Runtime;

const WORKSPACE_SYNC_TIMEOUT: Duration = Duration::from_secs(15);

async fn acquire_registration_demotion_barrier(
    ctx: &Ctx,
    registry: &RuntimeRegistry,
    workspace_id: &str,
    incoming_runtime_ids: &BTreeSet<String>,
    activity: Option<&Arc<DaemonActivity>>,
) -> anyhow::Result<Option<ClaimBarrierGuard>> {
    let Some(activity) = activity else {
        return Ok(None);
    };
    if !registry.registration_demotion_required(workspace_id, incoming_runtime_ids) {
        return Ok(None);
    }
    activity
        .pause_claims_until_idle(ctx)
        .await
        .map(Some)
        .ok_or_else(|| {
            anyhow::anyhow!("runtime demotion cancelled while draining workspace {workspace_id}")
        })
}

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

    /// Whether this built-in-only round differs from the last registration
    /// successfully applied for this workspace.
    fn builtin_registration_needed(&self, _workspace_id: &str) -> bool {
        true
    }

    /// Publishes provider-owned launch state in the same admission critical
    /// section as the authoritative registry. Callers publish launch state
    /// first so a newly visible runtime can always be resolved by a claimant.
    fn sampled_after_demotion_seq(&self) -> u64 {
        0
    }

    fn recovered_providers(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    /// Providers omitted from this payload because the probe was transient or
    /// because only the version-refresh owner may demote them safely.
    fn preserved_providers(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn demotable_providers(&self) -> BTreeMap<String, RuntimeVerdict> {
        BTreeMap::new()
    }

    /// Clears provider-owned launch state after the authoritative registry has
    /// removed confirmed unusable built-ins.
    fn demotion_applied(&self, _workspace_id: &str, _providers: &BTreeSet<String>) {}

    fn registration_applied(&self, _workspace_id: &str, _accepted: &[Runtime]) {}
}

#[async_trait::async_trait]
pub trait RuntimeRegistrationSource: Send + Sync + 'static {
    /// Starts one machine-level probe round. The returned object is reused for
    /// every workspace in this sync so N workspaces never cause N×M CLI probes.
    async fn begin_round(
        &self,
        ctx: Ctx,
        sampled_after_demotion_seq: u64,
    ) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>>;

    /// Probes built-in providers for the requested cadence. `None` means a
    /// real probe found no registration change; `Some` carries one shared
    /// built-in-only round for every tracked workspace.
    async fn begin_builtin_refresh(
        &self,
        ctx: Ctx,
        reason: BuiltinRefreshReason,
        sampled_after_demotion_seq: u64,
    ) -> anyhow::Result<Option<Arc<dyn RuntimeRegistrationRound>>>;

    /// Drops provider-owned launch state after workspace membership is no
    /// longer authoritative. Implementations without launch state can keep
    /// the default no-op.
    fn workspace_removed(&self, _workspace_id: &str) {}

    fn skipped_agents_snapshot(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
}

pub struct RuntimeRegistrationService<S: RuntimeRegistrationSource> {
    config: Arc<Config>,
    client: Arc<Client>,
    repo_state: Arc<DaemonRepoState>,
    repo_warmups: mpsc::Sender<RepoWarmupRequest>,
    source: Arc<S>,
    serial: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    pending_deregistrations: Mutex<PendingDeregistrations>,
}

#[derive(Default)]
struct PendingDeregistrations {
    ids: BTreeSet<String>,
    reasons: HashMap<String, crate::client::RuntimeOfflineReason>,
}

impl<S: RuntimeRegistrationSource> RuntimeRegistrationService<S> {
    pub(crate) fn new(
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
            pending_deregistrations: Mutex::new(PendingDeregistrations::default()),
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
        activity: Option<&Arc<DaemonActivity>>,
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
            let round = self.begin_round(ctx.child(), registry).await?;
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
                        activity,
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
            self.source.workspace_removed(workspace_id);
            registry.remove_workspace(workspace_id);
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
        let round = self.begin_round(ctx.child(), registry).await?;
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
            None,
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
        activity: &Arc<DaemonActivity>,
    ) -> anyhow::Result<()> {
        let workspace = registry
            .workspace(workspace_id)
            .ok_or_else(|| anyhow::anyhow!("workspace {workspace_id} is not tracked"))?;
        let round = self.begin_round(ctx.child(), registry).await?;
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
            Some(activity),
        )
        .await
    }

    pub async fn refresh_builtins_once(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        reason: BuiltinRefreshReason,
        activity: &Arc<DaemonActivity>,
    ) -> anyhow::Result<()> {
        let sampled_after = registry.demotion_seq_snapshot();
        let Some(round) = self
            .source
            .begin_builtin_refresh(ctx.child(), reason, sampled_after)
            .await?
        else {
            return Ok(());
        };
        registry.clear_recovered_providers(
            &round.recovered_providers(),
            round.sampled_after_demotion_seq(),
        );
        self.demote_unusable(ctx.child(), registry, Arc::clone(&round), activity)
            .await?;
        for workspace_id in registry.workspace_ids() {
            if !round.builtin_registration_needed(&workspace_id) {
                continue;
            }
            let Some(workspace) = registry.workspace(&workspace_id) else {
                continue;
            };
            if let Err(error) = self
                .register_builtin_workspace(
                    ctx.child(),
                    registry,
                    &workspace,
                    Arc::clone(&round),
                    activity,
                )
                .await
            {
                tracing::warn!(%workspace_id, %error, "built-in runtime refresh failed");
            }
        }
        Ok(())
    }

    async fn begin_round(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
    ) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>> {
        let sampled_after = registry.demotion_seq_snapshot();
        let round = self.source.begin_round(ctx, sampled_after).await?;
        registry.clear_recovered_providers(
            &round.recovered_providers(),
            round.sampled_after_demotion_seq(),
        );
        Ok(round)
    }

    async fn demote_unusable(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        round: Arc<dyn RuntimeRegistrationRound>,
        activity: &Arc<DaemonActivity>,
    ) -> anyhow::Result<()> {
        if let Err(error) = self
            .deregister_dropped(&ctx, registry, &[], HashMap::new())
            .await
        {
            tracing::warn!(%error, "pending provider deregistration retry failed");
        }
        let causes = round.demotable_providers();
        if causes.is_empty() {
            return Ok(());
        }
        let Some(_claim_barrier) = activity.try_claim_barrier() else {
            tracing::info!("provider demotion deferred while claims or tasks are active");
            return Ok(());
        };
        let partition = registry.demote_builtins(&causes);
        let providers: BTreeSet<String> = causes.keys().cloned().collect();
        for workspace_id in registry.workspace_ids() {
            round.demotion_applied(&workspace_id, &providers);
        }
        for (workspace_id, runtime_ids) in partition.demoted_by_workspace {
            let serial = self.workspace_lock(&workspace_id);
            let _guard = serial.lock().await;
            let reasons = partition
                .offline_reasons
                .iter()
                .filter(|(runtime_id, _)| runtime_ids.contains(runtime_id))
                .map(|(runtime_id, reason)| (runtime_id.clone(), reason.clone()))
                .collect();
            if let Err(error) = self
                .deregister_dropped(&ctx, registry, &runtime_ids, reasons)
                .await
            {
                tracing::warn!(%workspace_id, %error, "provider deregistration failed; continuing remaining workspaces");
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn register_workspace(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        workspace: &WorkspaceInfo,
        round: Arc<dyn RuntimeRegistrationRound>,
        recover_orphans: bool,
        allow_empty_refresh: bool,
        activity: Option<&Arc<DaemonActivity>>,
    ) -> anyhow::Result<()> {
        let payload = round
            .payload_for_workspace(ctx.child(), &workspace.id)
            .await?;
        let preserved_providers = round.preserved_providers();
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
            let incoming_runtime_ids: BTreeSet<String> = registry
                .workspace_runtimes(&workspace.id)
                .into_iter()
                .filter(|runtime| {
                    runtime.profile_id.is_empty() && preserved_providers.contains(&runtime.provider)
                })
                .map(|runtime| runtime.id)
                .collect();
            let _demotion_barrier = acquire_registration_demotion_barrier(
                &ctx,
                registry,
                &workspace.id,
                &incoming_runtime_ids,
                activity,
            )
            .await?;
            let serial = self.workspace_lock(&workspace.id);
            let _guard = serial.lock().await;
            let delta = registry.apply_registration_guarded(
                workspace.id.clone(),
                workspace.name.clone(),
                Vec::new(),
                &preserved_providers,
            )?;
            let accepted = registry.workspace_runtimes(&workspace.id);
            round.registration_applied(&workspace.id, &accepted);
            self.deregister_dropped(&ctx, registry, &delta.dropped, HashMap::new())
                .await?;
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
        let mut incoming_runtime_ids: BTreeSet<String> = runtime_ids.iter().cloned().collect();
        incoming_runtime_ids.extend(
            registry
                .workspace_runtimes(&workspace.id)
                .into_iter()
                .filter(|runtime| {
                    runtime.profile_id.is_empty() && preserved_providers.contains(&runtime.provider)
                })
                .map(|runtime| runtime.id),
        );
        let repos = response.repos;
        let settings = response.settings;
        {
            let _demotion_barrier = acquire_registration_demotion_barrier(
                &ctx,
                registry,
                &workspace.id,
                &incoming_runtime_ids,
                activity,
            )
            .await?;
            let serial = self.workspace_lock(&workspace.id);
            let _guard = serial.lock().await;
            let mut prospective = response.runtimes.clone();
            prospective.extend(
                registry
                    .workspace_runtimes(&workspace.id)
                    .into_iter()
                    .filter(|runtime| {
                        runtime.profile_id.is_empty()
                            && preserved_providers.contains(&runtime.provider)
                    }),
            );
            round.registration_applied(&workspace.id, &prospective);
            let delta = registry.apply_registration_guarded(
                workspace.id.clone(),
                workspace.name.clone(),
                response.runtimes,
                &preserved_providers,
            )?;
            self.deregister_dropped(&ctx, registry, &delta.dropped, HashMap::new())
                .await?;
            self.deregister_dropped(&ctx, registry, &delta.revived.ids, delta.revived.reasons)
                .await?;
        }
        self.repo_state
            .replace_workspace(&workspace.id, &repos, settings);
        if recover_orphans {
            let accepted_ids: BTreeSet<String> = registry
                .workspace_runtimes(&workspace.id)
                .into_iter()
                .map(|runtime| runtime.id)
                .collect();
            for runtime_id in runtime_ids {
                if !accepted_ids.contains(&runtime_id) {
                    continue;
                }
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
        activity: &Arc<DaemonActivity>,
    ) -> anyhow::Result<()> {
        let payload = round
            .payload_for_workspace(ctx.child(), &workspace.id)
            .await?;
        anyhow::ensure!(
            payload.failed_profiles.is_empty(),
            "built-in refresh returned profile failures for workspace {}",
            workspace.id
        );
        let preserved_providers = round.preserved_providers();
        let mut incoming_providers: BTreeSet<String> = payload
            .runtimes
            .iter()
            .filter_map(|runtime| runtime.get("type"))
            .map(|provider| provider.trim())
            .filter(|provider| !provider.is_empty())
            .map(str::to_string)
            .collect();
        incoming_providers.extend(preserved_providers.iter().cloned());
        if payload.runtimes.is_empty() {
            let _demotion_barrier =
                if registry.builtin_demotion_required(&workspace.id, &incoming_providers) {
                    Some(
                        activity
                            .pause_claims_until_idle(&ctx)
                            .await
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "built-in runtime demotion cancelled for workspace {}",
                                    workspace.id
                                )
                            })?,
                    )
                } else {
                    None
                };
            let serial = self.workspace_lock(&workspace.id);
            let _guard = serial.lock().await;
            // `Some(round)` means the provider completed an authoritative
            // changed probe. Applying an empty built-in set removes the last
            // vanished executable while preserving custom-profile runtimes.
            let delta = registry.apply_builtin_registration_guarded(
                &workspace.id,
                &workspace.name,
                Vec::new(),
                &preserved_providers,
            )?;
            let accepted = registry.workspace_runtimes(&workspace.id);
            round.registration_applied(&workspace.id, &accepted);
            self.deregister_dropped(&ctx, registry, &delta.dropped, HashMap::new())
                .await?;
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
        let _demotion_barrier =
            if registry.builtin_demotion_required(&workspace.id, &incoming_providers) {
                Some(
                    activity
                        .pause_claims_until_idle(&ctx)
                        .await
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "built-in runtime demotion cancelled for workspace {}",
                                workspace.id
                            )
                        })?,
                )
            } else {
                None
            };
        let serial = self.workspace_lock(&workspace.id);
        let _guard = serial.lock().await;
        let mut prospective = response.runtimes.clone();
        prospective.extend(
            registry
                .workspace_runtimes(&workspace.id)
                .into_iter()
                .filter(|runtime| {
                    runtime.profile_id.is_empty() && preserved_providers.contains(&runtime.provider)
                }),
        );
        round.registration_applied(&workspace.id, &prospective);
        let delta = registry.apply_builtin_registration_guarded(
            &workspace.id,
            &workspace.name,
            response.runtimes,
            &preserved_providers,
        )?;
        self.deregister_dropped(&ctx, registry, &delta.dropped, HashMap::new())
            .await?;
        self.deregister_dropped(&ctx, registry, &delta.revived.ids, delta.revived.reasons)
            .await?;
        Ok(())
    }

    async fn deregister_dropped(
        &self,
        ctx: &Ctx,
        registry: &RuntimeRegistry,
        runtime_ids: &[String],
        reasons: HashMap<String, crate::client::RuntimeOfflineReason>,
    ) -> anyhow::Result<()> {
        let requested = registry.untracked_runtime_ids(runtime_ids);
        let (pending_ids, pending_reasons) = {
            let mut pending = self.pending_deregistrations.lock().unwrap();
            pending.ids.extend(requested.iter().cloned());
            for (runtime_id, reason) in reasons {
                if pending.ids.contains(&runtime_id) {
                    pending.reasons.insert(runtime_id, reason);
                }
            }
            let still_untracked =
                registry.untracked_runtime_ids(&pending.ids.iter().cloned().collect::<Vec<_>>());
            pending.ids = still_untracked.iter().cloned().collect();
            let ids = pending.ids.clone();
            pending
                .reasons
                .retain(|runtime_id, _| ids.contains(runtime_id));
            let reasons = pending
                .reasons
                .iter()
                .filter(|(runtime_id, _)| pending.ids.contains(*runtime_id))
                .map(|(runtime_id, reason)| (runtime_id.clone(), reason.clone()))
                .collect();
            (still_untracked, reasons)
        };
        if pending_ids.is_empty() {
            return Ok(());
        }
        self.client
            .deregister(ctx, &pending_ids, pending_reasons)
            .await?;
        let mut pending = self.pending_deregistrations.lock().unwrap();
        for runtime_id in pending_ids {
            pending.ids.remove(&runtime_id);
            pending.reasons.remove(&runtime_id);
        }
        Ok(())
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

    pub(crate) fn skipped_agents_snapshot(&self) -> BTreeMap<String, String> {
        self.source.skipped_agents_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn runtime(id: &str, provider: &str) -> crate::types::Runtime {
        crate::types::Runtime {
            id: id.to_string(),
            provider: provider.to_string(),
            ..crate::types::Runtime::default()
        }
    }

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

    #[tokio::test]
    async fn custom_profile_demotion_waits_for_active_tasks_and_blocks_claims() {
        let registry = Arc::new(RuntimeRegistry::new(Arc::new(
            crate::runtime_set::RuntimeSet::new(),
        )));
        registry
            .apply_registration(
                "workspace-1",
                "One",
                vec![runtime("custom-runtime", "codex")],
            )
            .unwrap();
        let activity = DaemonActivity::new();
        let claim = activity.try_enter_claim().unwrap();
        let tasks = claim.handoff(vec![Vec::new()]).await;
        let acquiring = tokio::spawn({
            let registry = Arc::clone(&registry);
            let activity = Arc::clone(&activity);
            async move {
                let ctx = Ctx::new();
                acquire_registration_demotion_barrier(
                    &ctx,
                    &registry,
                    "workspace-1",
                    &BTreeSet::new(),
                    Some(&activity),
                )
                .await
            }
        });

        tokio::task::yield_now().await;
        assert!(activity.claims_paused());
        assert!(activity.try_enter_claim().is_none());
        assert!(!acquiring.is_finished());
        drop(tasks);

        let barrier = acquiring.await.unwrap().unwrap().unwrap();
        assert!(activity.claims_paused());
        drop(barrier);
        assert!(!activity.claims_paused());
    }

    #[tokio::test]
    async fn unchanged_registration_does_not_pause_claims() {
        let registry = RuntimeRegistry::new(Arc::new(crate::runtime_set::RuntimeSet::new()));
        registry
            .apply_registration("workspace-1", "One", vec![runtime("runtime-1", "codex")])
            .unwrap();
        let activity = DaemonActivity::new();
        let ctx = Ctx::new();

        let barrier = acquire_registration_demotion_barrier(
            &ctx,
            &registry,
            "workspace-1",
            &BTreeSet::from(["runtime-1".to_string()]),
            Some(&activity),
        )
        .await
        .unwrap();

        assert!(barrier.is_none());
        assert!(!activity.claims_paused());
        assert!(activity.try_enter_claim().is_some());
    }
}
