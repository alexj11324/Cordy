//! Ordered workspace/runtime registration orchestration.
//!
//! Provider integration supplies one machine-level probe round and derives
//! each workspace's custom-profile payload. The daemon owns API ordering,
//! authoritative state application, dropped-row cleanup, orphan recovery, and
//! membership removal.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::activity::DaemonActivity;
use crate::agents_refresh::{ConvergeRetryState, RuntimeVerdict};
use crate::client::{Client, RuntimeOfflineReason, WorkspaceInfo};
use crate::config::Config;
use crate::provider_registration::profile_set_signature;
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
    /// Stable signature of the successfully fetched profile set. `None` means
    /// the profile endpoint was unavailable, so a transient failure must not
    /// overwrite the last known signature.
    pub profile_set_signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinRefreshReason {
    Discovery,
    Version,
}

/// Result of one machine-level built-in refresh round. `attempted` counts
/// workspaces whose live state said a registration was needed; `progressed`
/// counts accepted responses. The distinction keeps a failed workspace in
/// the next discovery round's retry set instead of treating a completed probe
/// as convergence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuiltinRefreshOutcome {
    pub attempted: usize,
    pub progressed: usize,
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
    /// A successful empty payload is authoritative for full workspace/profile
    /// convergence. Built-in refresh rounds handle an empty probe as a
    /// transient/no-provider observation and preserve the current runtime set.
    /// Probe/fetch failures must be returned as `Err` rather than disguised as
    /// empty.
    async fn payload_for_workspace(
        &self,
        ctx: Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<RegistrationPayload>;

    /// Publishes provider-owned launch state only after the corresponding
    /// server response has been accepted into the authoritative registry.
    /// Failed register calls never invoke this hook.
    fn registration_applied(&self, workspace_id: &str);

    /// Whether this machine probe discovered a provider not present in the
    /// previously published availability set. Discovery gains bypass the
    /// convergence backoff, matching Go's immediate first attempt rule.
    fn gained_providers(&self) -> bool {
        false
    }

    /// Confirmed provider verdicts from this probe round. Only the periodic
    /// version path may act on them because demotion must hold the claim
    /// barrier while removing runtime identities.
    fn demotable_providers(&self) -> BTreeMap<String, RuntimeVerdict> {
        BTreeMap::new()
    }

    /// Providers that probed successfully in this round and may release an
    /// older demotion hold, subject to the registry's generation fence.
    fn recovered_providers(&self) -> Vec<String> {
        Vec::new()
    }

    /// Providers omitted from this round for a transient/confirmed probe
    /// reason. Full registration preserves their already accepted rows;
    /// built-in refresh uses additive merge instead.
    fn preserve_providers(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

#[async_trait::async_trait]
pub trait RuntimeRegistrationSource: Send + Sync + 'static {
    /// Starts one machine-level probe round. The returned object is reused for
    /// every workspace in this sync so N workspaces never cause N×M CLI probes.
    async fn begin_round(&self, ctx: Ctx) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>>;

    /// Probes built-in providers for the requested cadence. `Some` carries one
    /// shared built-in-only round for every tracked workspace; the round itself
    /// decides whether each workspace is missing a provider or version.
    async fn begin_builtin_refresh(
        &self,
        ctx: Ctx,
        reason: BuiltinRefreshReason,
    ) -> anyhow::Result<Option<Arc<dyn RuntimeRegistrationRound>>>;

    /// Performs the cheap availability half of a discovery tick. `None`
    /// preserves the legacy behavior for sources that do not expose a
    /// separate availability probe; production provider registration returns
    /// `Some` and the service then gates version probes on live missing state.
    async fn refresh_builtin_availability(&self, _ctx: Ctx) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Releases provider-owned launch state when workspace membership is
    /// removed. A later re-add must not revive stale custom profile commands
    /// if its first profile fetch fails.
    fn workspace_removed(&self, workspace_id: &str);

    /// Returns the source-owned machine discovery state for `/health`.
    /// Implementations that do not own provider discovery return `None`; the
    /// production provider source supplies the copy-on-write availability set
    /// and current skip reasons.
    fn health_snapshot(&self) -> Option<(Vec<String>, HashMap<String, String>)> {
        None
    }
}

pub struct RuntimeRegistrationService<S: RuntimeRegistrationSource> {
    config: Arc<Config>,
    client: Arc<Client>,
    repo_state: Arc<DaemonRepoState>,
    repo_warmups: mpsc::Sender<RepoWarmupRequest>,
    source: Arc<S>,
    serial: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    pending_deregistrations: PendingDeregistrations,
    deregistration_flush: AsyncMutex<()>,
    builtin_retry: Mutex<ConvergeRetryState>,
    activity: Mutex<Option<Arc<DaemonActivity>>>,
}

#[derive(Default)]
struct PendingDeregistrations {
    runtime_ids: Mutex<BTreeSet<String>>,
    reasons: Mutex<HashMap<String, RuntimeOfflineReason>>,
}

impl PendingDeregistrations {
    fn queue(&self, runtime_ids: &[String]) {
        self.queue_with_reasons(runtime_ids, HashMap::new());
    }

    fn queue_with_reasons(
        &self,
        runtime_ids: &[String],
        reasons: HashMap<String, RuntimeOfflineReason>,
    ) {
        self.runtime_ids
            .lock()
            .unwrap()
            .extend(runtime_ids.iter().filter(|id| !id.is_empty()).cloned());
        self.reasons.lock().unwrap().extend(
            reasons
                .into_iter()
                .filter(|(runtime_id, _)| !runtime_id.is_empty()),
        );
    }

    fn snapshot(&self) -> Vec<String> {
        self.runtime_ids.lock().unwrap().iter().cloned().collect()
    }

    fn acknowledge(&self, runtime_ids: &[String]) {
        let mut pending = self.runtime_ids.lock().unwrap();
        for runtime_id in runtime_ids {
            pending.remove(runtime_id);
        }
        let mut reasons = self.reasons.lock().unwrap();
        for runtime_id in runtime_ids {
            reasons.remove(runtime_id);
        }
    }

    fn reasons(&self, runtime_ids: &[String]) -> HashMap<String, RuntimeOfflineReason> {
        let reasons = self.reasons.lock().unwrap();
        runtime_ids
            .iter()
            .filter_map(|runtime_id| {
                reasons
                    .get(runtime_id)
                    .cloned()
                    .map(|reason| (runtime_id.clone(), reason))
            })
            .collect()
    }
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
            pending_deregistrations: PendingDeregistrations::default(),
            deregistration_flush: AsyncMutex::new(()),
            builtin_retry: Mutex::new(ConvergeRetryState::new()),
            activity: Mutex::new(None),
        }
    }

    pub(crate) fn set_activity(&self, activity: Arc<DaemonActivity>) {
        *self.activity.lock().unwrap() = Some(activity);
    }

    /// Delegates the provider source's machine discovery state to the health
    /// owner without exposing the source itself to the stack.
    pub fn health_snapshot(&self) -> Option<(Vec<String>, HashMap<String, String>)> {
        self.source.health_snapshot()
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
        if let Err(error) = self.flush_pending_deregistrations(&ctx).await {
            tracing::warn!(%error, "pending runtime deregistration retry failed");
        }
        let workspaces =
            tokio::time::timeout(WORKSPACE_SYNC_TIMEOUT, self.client.list_workspaces(&ctx))
                .await
                .map_err(|_| anyhow::anyhow!("list workspaces timed out"))??;
        let api_ids: BTreeSet<String> = workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect();
        let tracked: BTreeSet<String> = registry.workspace_ids().into_iter().collect();
        let mut needs_registration = Vec::new();
        for workspace in &workspaces {
            let is_tracked = tracked.contains(&workspace.id);
            let needs_recovery = registry.workspace_needs_runtime_recovery(&workspace.id);
            let profile_changed = if reconcile_profiles && is_tracked {
                match self
                    .workspace_profiles_changed(&ctx, registry, &workspace.id)
                    .await
                {
                    Ok(changed) => changed,
                    Err(error) => {
                        // A reconnect-time profile read is best effort. The
                        // normal recovery path below still runs, but a
                        // transient profile endpoint failure must not turn
                        // into an empty registration or force a re-register.
                        tracing::debug!(
                            workspace_id = %workspace.id,
                            %error,
                            "workspace profile reconcile failed"
                        );
                        false
                    }
                }
            } else {
                false
            };

            // Go refreshes tracked profiles only after the signature changes;
            // ordinary membership sync and empty-runtime recovery remain
            // independent of the profile notification path.
            if !is_tracked || needs_recovery || profile_changed {
                needs_registration.push((
                    workspace.clone(),
                    !is_tracked || (needs_recovery && !profile_changed),
                    profile_changed,
                ));
            }
        }

        let mut registered = 0usize;
        if !needs_registration.is_empty() {
            let demotion_generation = registry.demotion_generation();
            let round = self.source.begin_round(ctx.child()).await?;
            registry.clear_provider_demotions(&round.recovered_providers(), demotion_generation);
            for (workspace, recover_orphans, profile_changed) in &needs_registration {
                match self
                    .register_workspace(
                        ctx.child(),
                        registry,
                        workspace,
                        Arc::clone(&round),
                        *recover_orphans,
                        *profile_changed,
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
            let dropped = registry.remove_workspace(workspace_id);
            self.source.workspace_removed(workspace_id);
            self.repo_state.remove_workspace(workspace_id);
            self.pending_deregistrations.queue(&dropped);
            if let Err(error) = self.flush_pending_deregistrations(&ctx).await {
                tracing::warn!(
                    %workspace_id,
                    runtimes = dropped.len(),
                    %error,
                    "removed workspace runtime deregistration deferred"
                );
            }
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

    async fn workspace_profiles_changed(
        &self,
        ctx: &Ctx,
        registry: &RuntimeRegistry,
        workspace_id: &str,
    ) -> anyhow::Result<bool> {
        let response = tokio::time::timeout(
            WORKSPACE_SYNC_TIMEOUT,
            self.client.get_runtime_profiles(ctx, workspace_id),
        )
        .await
        .map_err(|_| anyhow::anyhow!("runtime profile list timed out"))??;
        let live = profile_set_signature(&response.runtime_profiles);
        Ok(registry
            .workspace_profile_signature(workspace_id)
            .is_none_or(|cached| cached != live))
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
    ) -> anyhow::Result<BuiltinRefreshOutcome> {
        let gained_hint = if reason == BuiltinRefreshReason::Discovery {
            let Some(gained) = self
                .source
                .refresh_builtin_availability(ctx.child())
                .await?
            else {
                false
            };

            // Go's discovery loop checks the live availability set before it
            // enters the expensive convergence half. This also avoids probing
            // an empty workspace set, while preserving the additive health
            // state even when no workspace currently needs registration.
            if let Some((agents, _)) = self.source.health_snapshot() {
                let available = agents.into_iter().collect::<BTreeSet<_>>();
                if !registry.any_workspace_missing_builtin(&available) {
                    self.builtin_retry.lock().unwrap().reset_backoff();
                    return Ok(BuiltinRefreshOutcome::default());
                }
            }
            gained
        } else {
            false
        };
        let demotion_generation = registry.demotion_generation();
        let Some(round) = self
            .source
            .begin_builtin_refresh(ctx.child(), reason)
            .await?
        else {
            return Ok(BuiltinRefreshOutcome::default());
        };
        registry.clear_provider_demotions(&round.recovered_providers(), demotion_generation);
        if reason == BuiltinRefreshReason::Version {
            let demotable = round.demotable_providers();
            if !demotable.is_empty() {
                self.demote_unusable_runtimes(ctx.child(), registry, &demotable)
                    .await;
            }
        }
        let now = Instant::now();
        let discovery_allowed = reason != BuiltinRefreshReason::Discovery
            || self
                .builtin_retry
                .lock()
                .unwrap()
                .should_attempt(gained_hint || round.gained_providers(), now);
        if !discovery_allowed {
            return Ok(BuiltinRefreshOutcome::default());
        }
        let mut outcome = BuiltinRefreshOutcome::default();
        for workspace_id in registry.workspace_ids() {
            let Some(workspace) = registry.workspace(&workspace_id) else {
                continue;
            };
            match self
                .register_builtin_workspace(ctx.child(), registry, &workspace, Arc::clone(&round))
                .await
            {
                Ok(true) => {
                    outcome.attempted += 1;
                    outcome.progressed += 1;
                }
                Ok(false) => {}
                Err(error) => {
                    // The workspace was selected from live state and the
                    // request was therefore needed, even when the payload
                    // fetch or register call failed before an accepted
                    // response. Keep it counted as attempted so the caller
                    // can apply its retry policy.
                    outcome.attempted += 1;
                    tracing::warn!(%workspace_id, %error, "built-in runtime refresh failed");
                }
            }
        }
        if reason == BuiltinRefreshReason::Discovery {
            let progressed = outcome.attempted == 0 || outcome.progressed > 0;
            self.builtin_retry
                .lock()
                .unwrap()
                .record_attempt(progressed, now);
        }
        Ok(outcome)
    }

    async fn demote_unusable_runtimes(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        causes: &BTreeMap<String, RuntimeVerdict>,
    ) {
        let Some(activity) = self.activity.lock().unwrap().clone() else {
            tracing::error!("cannot demote unusable runtimes before activity is installed");
            return;
        };
        if !activity.try_set_claim_barrier() {
            tracing::info!(providers = ?causes.keys().collect::<Vec<_>>(), "defer runtime demotion: task or claim in flight");
            return;
        }

        // Hold every workspace registration serial while the registry removes
        // rows and the server receives the matching deregistration. This keeps
        // an in-flight full/profile refresh from reviving a condemned runtime
        // between the local demotion and cleanup.
        let workspace_ids = registry.workspace_ids();
        let locks: Vec<Arc<AsyncMutex<()>>> = workspace_ids
            .iter()
            .map(|workspace_id| self.workspace_lock(workspace_id))
            .collect();
        let mut guards = Vec::with_capacity(workspace_ids.len());
        for lock in &locks {
            guards.push(lock.lock().await);
        }

        let delta = registry.demote_builtins(causes);
        if !delta.dropped.is_empty() {
            let reasons = delta.offline_reasons.clone();
            self.pending_deregistrations
                .queue_with_reasons(&delta.dropped, reasons);
            if let Err(error) = self.flush_pending_deregistrations(&ctx).await {
                tracing::warn!(
                    runtimes = delta.dropped.len(),
                    %error,
                    "demoted runtime deregistration deferred"
                );
            }
            tracing::warn!(
                providers = ?causes.keys().collect::<Vec<_>>(),
                runtimes = delta.dropped.len(),
                "agent CLI is no longer usable; taking runtimes offline"
            );
        }

        drop(guards);
        activity.release_claim_barrier();
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
            let delta = registry.apply_registration_with_profile_signature(
                workspace.id.clone(),
                workspace.name.clone(),
                Vec::new(),
                payload.profile_set_signature.as_deref(),
            )?;
            round.registration_applied(&workspace.id);
            self.queue_and_flush_dropped(&ctx, &delta.dropped).await;
            tracing::info!(
                workspace_id = %workspace.id,
                dropped = delta.dropped.len(),
                "workspace runtime profiles converged to zero"
            );
            return Ok(());
        }
        let (runtime_ids, repos, settings, delta) = {
            let preserve_providers = round.preserve_providers();
            // A failed deregistration may be retried while this workspace is
            // being re-added. Fence the server register and authoritative
            // apply together so an old retry cannot take a newly accepted,
            // server-reused runtime ID offline after it becomes active again.
            let _fence = self.deregistration_flush.lock().await;
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
            let delta = match registry
                .apply_registration_preserving_builtins_with_profile_signature(
                    workspace.id.clone(),
                    workspace.name.clone(),
                    response.runtimes,
                    &preserve_providers,
                    payload.profile_set_signature.as_deref(),
                ) {
                Ok(delta) => delta,
                Err(error) => {
                    // The server already accepted these rows. Retain them for
                    // cleanup because the local registry rejected the reply.
                    self.pending_deregistrations.queue(&runtime_ids);
                    return Err(error);
                }
            };
            self.pending_deregistrations.acknowledge(&runtime_ids);
            registry.record_builtin_versions(&workspace.id, &payload.runtimes);
            round.registration_applied(&workspace.id);
            (runtime_ids, repos, settings, delta)
        };
        self.repo_state
            .replace_workspace(&workspace.id, &repos, settings);
        let mut cleanup = delta.dropped.clone();
        cleanup.extend(delta.revived.clone());
        let reasons = delta.offline_reasons.clone();
        self.queue_and_flush_dropped_with_reasons(&ctx, &cleanup, reasons)
            .await;
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
    ) -> anyhow::Result<bool> {
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
            // A discovery/version probe can be empty because a CLI is
            // temporarily unreadable or because no provider is installed.
            // Go keeps existing runtime rows in this path; only an explicit
            // workspace/profile convergence owns authoritative empty state.
            return Ok(false);
        }
        if !registry.workspace_needs_builtin_refresh(&workspace.id, &payload.runtimes) {
            return Ok(false);
        }
        let delta = {
            let _fence = self.deregistration_flush.lock().await;
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
            let runtime_ids = response
                .runtimes
                .iter()
                .map(|runtime| runtime.id.clone())
                .collect::<Vec<_>>();
            let delta = match registry.merge_builtin_registration(
                &workspace.id,
                &workspace.name,
                response.runtimes,
            ) {
                Ok(delta) => delta,
                Err(error) => {
                    self.pending_deregistrations.queue(&runtime_ids);
                    return Err(error);
                }
            };
            self.pending_deregistrations.acknowledge(&runtime_ids);
            registry.record_builtin_versions(&workspace.id, &payload.runtimes);
            round.registration_applied(&workspace.id);
            delta
        };
        let mut cleanup = delta.dropped.clone();
        cleanup.extend(delta.revived.clone());
        let reasons = delta.offline_reasons.clone();
        self.queue_and_flush_dropped_with_reasons(&ctx, &cleanup, reasons)
            .await;
        Ok(true)
    }

    /// Final best-effort delivery for rows dropped before the daemon's current
    /// runtime set. The caller supplies a fresh context because the daemon root
    /// is already cancelled during shutdown.
    pub(crate) async fn flush_deregistrations(&self, ctx: &Ctx) -> anyhow::Result<()> {
        self.flush_pending_deregistrations(ctx).await
    }

    async fn queue_and_flush_dropped(&self, ctx: &Ctx, runtime_ids: &[String]) {
        self.queue_and_flush_dropped_with_reasons(ctx, runtime_ids, HashMap::new())
            .await;
    }

    async fn queue_and_flush_dropped_with_reasons(
        &self,
        ctx: &Ctx,
        runtime_ids: &[String],
        reasons: HashMap<String, RuntimeOfflineReason>,
    ) {
        self.pending_deregistrations
            .queue_with_reasons(runtime_ids, reasons);
        if let Err(error) = self.flush_pending_deregistrations(ctx).await {
            tracing::warn!(
                runtimes = runtime_ids.len(),
                %error,
                "runtime deregistration deferred"
            );
        }
    }

    async fn flush_pending_deregistrations(&self, ctx: &Ctx) -> anyhow::Result<()> {
        let _flush = self.deregistration_flush.lock().await;
        let runtime_ids = self.pending_deregistrations.snapshot();
        if runtime_ids.is_empty() {
            return Ok(());
        }
        let reasons = self.pending_deregistrations.reasons(&runtime_ids);
        self.client.deregister(ctx, &runtime_ids, reasons).await?;
        self.pending_deregistrations.acknowledge(&runtime_ids);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn pending_deregistrations_are_deduplicated_and_acknowledged_by_snapshot() {
        let pending = PendingDeregistrations::default();
        pending.queue(&[
            "runtime-2".to_string(),
            "runtime-1".to_string(),
            "runtime-2".to_string(),
            String::new(),
        ]);

        let first = pending.snapshot();
        assert_eq!(
            first,
            vec!["runtime-1".to_string(), "runtime-2".to_string()]
        );

        pending.queue(&["runtime-3".to_string()]);
        pending.acknowledge(&first);
        assert_eq!(pending.snapshot(), vec!["runtime-3".to_string()]);
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
            profile_set_signature: None,
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
