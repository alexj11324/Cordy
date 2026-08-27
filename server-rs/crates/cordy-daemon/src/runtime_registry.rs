//! Authoritative daemon workspace/runtime registration state.
//!
//! Every registration response is applied under one lock and publishes the
//! resulting complete [`RuntimeSet`] before releasing ownership. Runtime
//! lookup, runtime-gone recovery, health, and control transport therefore
//! cannot observe independently maintained identity maps.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use crate::agents_refresh::{
    partition_demotable_runtimes, DemotionPartition, RevivedRuntimes, RuntimeVerdict,
};
use crate::health::HealthWorkspace;
use crate::runtime_set::RuntimeSet;
use crate::types::{Runtime, RuntimeExecutionTarget};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRuntimeState {
    pub id: String,
    pub name: String,
    pub runtime_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationDelta {
    pub added: Vec<String>,
    pub dropped: Vec<String>,
    pub revived: RevivedRuntimes,
}

struct DemotionRecord {
    verdict: RuntimeVerdict,
    seq: u64,
}

#[derive(Default)]
struct RegistryState {
    workspaces: BTreeMap<String, WorkspaceRuntimeState>,
    runtimes: BTreeMap<String, Runtime>,
    runtime_workspaces: BTreeMap<String, String>,
    demoted_providers: BTreeMap<String, DemotionRecord>,
    demotion_seq: u64,
}

pub struct RuntimeRegistry {
    state: RwLock<RegistryState>,
    runtime_set: Arc<RuntimeSet>,
}

impl RuntimeRegistry {
    pub fn new(runtime_set: Arc<RuntimeSet>) -> Self {
        Self {
            state: RwLock::new(RegistryState::default()),
            runtime_set,
        }
    }

    /// Replaces one workspace's accepted runtime rows as a single ordered
    /// registration commit. Duplicate/empty IDs and IDs owned by another
    /// workspace are rejected without changing state.
    pub fn apply_registration(
        &self,
        workspace_id: impl Into<String>,
        workspace_name: impl Into<String>,
        runtimes: Vec<Runtime>,
    ) -> anyhow::Result<RegistrationDelta> {
        self.apply_registration_guarded(
            workspace_id,
            workspace_name,
            runtimes,
            &BTreeSet::new(),
        )
    }

    pub(crate) fn apply_registration_guarded(
        &self,
        workspace_id: impl Into<String>,
        workspace_name: impl Into<String>,
        mut runtimes: Vec<Runtime>,
        preserved_builtin_providers: &BTreeSet<String>,
    ) -> anyhow::Result<RegistrationDelta> {
        let workspace_id = workspace_id.into();
        anyhow::ensure!(!workspace_id.is_empty(), "workspace id is required");

        let mut wire_ids = BTreeSet::new();
        for runtime in &runtimes {
            anyhow::ensure!(!runtime.id.is_empty(), "registered runtime id is required");
            anyhow::ensure!(
                !runtime.provider.is_empty(),
                "registered runtime {} has no provider",
                runtime.id
            );
            anyhow::ensure!(
                wire_ids.insert(runtime.id.clone()),
                "registration returned duplicate runtime id {}",
                runtime.id
            );
        }

        let mut state = self.state.write().unwrap();
        let mut revived = RevivedRuntimes::default();
        runtimes.retain(|runtime| {
            if !runtime.profile_id.is_empty() {
                return true;
            }
            let Some(record) = state.demoted_providers.get(&runtime.provider) else {
                return true;
            };
            revived.ids.push(runtime.id.clone());
            if let Some(reason) = &record.verdict.offline {
                revived.reasons.insert(runtime.id.clone(), reason.clone());
            }
            false
        });

        let incoming_builtin_providers: BTreeSet<String> = runtimes
            .iter()
            .filter(|runtime| runtime.profile_id.is_empty())
            .map(|runtime| runtime.provider.clone())
            .collect();
        let preserved: Vec<Runtime> = state
            .workspaces
            .get(&workspace_id)
            .into_iter()
            .flat_map(|workspace| workspace.runtime_ids.iter())
            .filter_map(|runtime_id| state.runtimes.get(runtime_id))
            .filter(|runtime| {
                runtime.profile_id.is_empty()
                    && preserved_builtin_providers.contains(&runtime.provider)
                    && !incoming_builtin_providers.contains(&runtime.provider)
                    && !state.demoted_providers.contains_key(&runtime.provider)
            })
            .cloned()
            .collect();
        runtimes.extend(preserved);

        let incoming_ids: BTreeSet<String> = runtimes
            .iter()
            .map(|runtime| runtime.id.clone())
            .collect();
        for runtime_id in &incoming_ids {
            if let Some(owner) = state.runtime_workspaces.get(runtime_id) {
                anyhow::ensure!(
                    owner == &workspace_id,
                    "runtime {runtime_id} is already owned by workspace {owner}"
                );
            }
        }

        let previous_ids: BTreeSet<String> = state
            .workspaces
            .get(&workspace_id)
            .map(|workspace| workspace.runtime_ids.iter().cloned().collect())
            .unwrap_or_default();
        let added = incoming_ids.difference(&previous_ids).cloned().collect();
        let dropped: Vec<String> = previous_ids.difference(&incoming_ids).cloned().collect();

        for runtime_id in &previous_ids {
            state.runtimes.remove(runtime_id);
            state.runtime_workspaces.remove(runtime_id);
        }
        for runtime in runtimes {
            state
                .runtime_workspaces
                .insert(runtime.id.clone(), workspace_id.clone());
            state.runtimes.insert(runtime.id.clone(), runtime);
        }
        state.workspaces.insert(
            workspace_id.clone(),
            WorkspaceRuntimeState {
                id: workspace_id,
                name: workspace_name.into(),
                runtime_ids: incoming_ids.into_iter().collect(),
            },
        );
        publish_runtime_set(&state, &self.runtime_set);
        Ok(RegistrationDelta {
            added,
            dropped,
            revived,
        })
    }

    /// Applies a built-in-only registration response while preserving custom
    /// profile runtimes already tracked for the workspace. The caller holds
    /// the workspace registration serial across the server request and this
    /// merge, so a profile refresh cannot interleave between the snapshot and
    /// authoritative apply.
    pub fn apply_builtin_registration(
        &self,
        workspace_id: &str,
        workspace_name: &str,
        builtins: Vec<Runtime>,
    ) -> anyhow::Result<RegistrationDelta> {
        self.apply_builtin_registration_guarded(
            workspace_id,
            workspace_name,
            builtins,
            &BTreeSet::new(),
        )
    }

    pub(crate) fn apply_builtin_registration_guarded(
        &self,
        workspace_id: &str,
        workspace_name: &str,
        builtins: Vec<Runtime>,
        preserved_builtin_providers: &BTreeSet<String>,
    ) -> anyhow::Result<RegistrationDelta> {
        anyhow::ensure!(
            builtins.iter().all(|runtime| runtime.profile_id.is_empty()),
            "built-in registration returned a custom profile runtime"
        );
        let custom_runtimes: Vec<Runtime> = {
            let state = self.state.read().unwrap();
            let runtime_ids = state
                .workspaces
                .get(workspace_id)
                .map(|workspace| workspace.runtime_ids.as_slice())
                .unwrap_or_default();
            runtime_ids
                .iter()
                .filter_map(|runtime_id| state.runtimes.get(runtime_id))
                .filter(|runtime| !runtime.profile_id.is_empty())
                .cloned()
                .collect()
        };
        let mut combined = builtins;
        combined.extend(custom_runtimes);
        self.apply_registration_guarded(
            workspace_id,
            workspace_name,
            combined,
            preserved_builtin_providers,
        )
    }

    /// Removes one runtime after a server `runtime_gone` event. Returns its
    /// workspace so the caller can serialize and retry registration there.
    pub fn remove_runtime(&self, runtime_id: &str) -> Option<String> {
        let mut state = self.state.write().unwrap();
        let workspace_id = state.runtime_workspaces.remove(runtime_id)?;
        state.runtimes.remove(runtime_id);
        if let Some(workspace) = state.workspaces.get_mut(&workspace_id) {
            workspace.runtime_ids.retain(|id| id != runtime_id);
        }
        publish_runtime_set(&state, &self.runtime_set);
        Some(workspace_id)
    }

    /// Removes a workspace the account no longer belongs to and returns the
    /// runtime IDs that must be deregistered server-side.
    pub fn remove_workspace(&self, workspace_id: &str) -> Vec<String> {
        let mut state = self.state.write().unwrap();
        let Some(workspace) = state.workspaces.remove(workspace_id) else {
            return Vec::new();
        };
        for runtime_id in &workspace.runtime_ids {
            state.runtimes.remove(runtime_id);
            state.runtime_workspaces.remove(runtime_id);
        }
        publish_runtime_set(&state, &self.runtime_set);
        workspace.runtime_ids
    }

    pub fn provider_for_runtime(&self, runtime_id: &str) -> Option<String> {
        self.state
            .read()
            .unwrap()
            .runtimes
            .get(runtime_id)
            .map(|runtime| runtime.provider.clone())
    }

    /// Returns the complete identity selected by the accepted registration
    /// row. Custom profile IDs cannot be reconstructed from a provider string
    /// after claim, so the task path must carry both values together.
    pub fn execution_target_for_runtime(&self, runtime_id: &str) -> Option<RuntimeExecutionTarget> {
        self.state
            .read()
            .unwrap()
            .runtimes
            .get(runtime_id)
            .map(|runtime| RuntimeExecutionTarget {
                provider: runtime.provider.clone(),
                profile_id: runtime.profile_id.clone(),
            })
    }

    pub fn workspace_for_runtime(&self, runtime_id: &str) -> Option<String> {
        self.state
            .read()
            .unwrap()
            .runtime_workspaces
            .get(runtime_id)
            .cloned()
    }

    pub fn runtime_ids(&self) -> Vec<String> {
        self.runtime_set.snapshot()
    }

    pub fn health_workspaces(&self) -> Vec<HealthWorkspace> {
        self.state
            .read()
            .unwrap()
            .workspaces
            .values()
            .map(|workspace| HealthWorkspace {
                id: workspace.id.clone(),
                runtimes: workspace.runtime_ids.clone(),
            })
            .collect()
    }

    pub fn workspace_ids(&self) -> Vec<String> {
        self.state
            .read()
            .unwrap()
            .workspaces
            .keys()
            .cloned()
            .collect()
    }

    pub fn workspace(&self, workspace_id: &str) -> Option<WorkspaceRuntimeState> {
        self.state
            .read()
            .unwrap()
            .workspaces
            .get(workspace_id)
            .cloned()
    }

    pub(crate) fn workspace_runtimes(&self, workspace_id: &str) -> Vec<Runtime> {
        let state = self.state.read().unwrap();
        state
            .workspaces
            .get(workspace_id)
            .into_iter()
            .flat_map(|workspace| workspace.runtime_ids.iter())
            .filter_map(|runtime_id| state.runtimes.get(runtime_id))
            .cloned()
            .collect()
    }

    pub(crate) fn untracked_runtime_ids(&self, runtime_ids: &[String]) -> Vec<String> {
        let state = self.state.read().unwrap();
        runtime_ids
            .iter()
            .filter(|runtime_id| !state.runtimes.contains_key(runtime_id.as_str()))
            .cloned()
            .collect()
    }

    pub(crate) fn demotion_seq_snapshot(&self) -> u64 {
        self.state.read().unwrap().demotion_seq
    }

    /// Releases only holds that are no newer than the probe that observed the
    /// provider healthy. A slower, older probe can never clear a newer verdict.
    pub(crate) fn clear_recovered_providers(
        &self,
        providers: &BTreeSet<String>,
        sampled_after: u64,
    ) {
        let mut state = self.state.write().unwrap();
        state.demoted_providers.retain(|provider, record| {
            !providers.contains(provider) || record.seq > sampled_after
        });
    }

    /// Applies a confirmed machine-provider verdict as one local commit. The
    /// caller owns the global claim barrier while this runs and while the
    /// resulting rows are deregistered under their workspace serials.
    pub(crate) fn demote_builtins(
        &self,
        causes: &BTreeMap<String, RuntimeVerdict>,
    ) -> DemotionPartition {
        if causes.is_empty() {
            return DemotionPartition::default();
        }
        let mut state = self.state.write().unwrap();
        state.demotion_seq = state.demotion_seq.saturating_add(1);
        let seq = state.demotion_seq;
        for (provider, verdict) in causes {
            state.demoted_providers.insert(
                provider.clone(),
                DemotionRecord {
                    verdict: verdict.clone(),
                    seq,
                },
            );
        }
        let workspaces: BTreeMap<String, Vec<String>> = state
            .workspaces
            .iter()
            .map(|(id, workspace)| (id.clone(), workspace.runtime_ids.clone()))
            .collect();
        let (kept, partition) =
            partition_demotable_runtimes(&workspaces, &state.runtimes, causes);
        for runtime_id in &partition.demoted_ids {
            state.runtimes.remove(runtime_id);
            state.runtime_workspaces.remove(runtime_id);
        }
        for (workspace_id, runtime_ids) in kept {
            if let Some(workspace) = state.workspaces.get_mut(&workspace_id) {
                workspace.runtime_ids = runtime_ids;
            }
        }
        publish_runtime_set(&state, &self.runtime_set);
        partition
    }

    /// Reports whether an authoritative built-in refresh omits any provider
    /// family currently registered for this workspace. Custom-profile rows
    /// are deliberately excluded: built-in refresh preserves them.
    pub(crate) fn builtin_demotion_required(
        &self,
        workspace_id: &str,
        incoming_providers: &BTreeSet<String>,
    ) -> bool {
        let state = self.state.read().unwrap();
        let Some(workspace) = state.workspaces.get(workspace_id) else {
            return false;
        };
        workspace
            .runtime_ids
            .iter()
            .filter_map(|runtime_id| state.runtimes.get(runtime_id))
            .filter(|runtime| runtime.profile_id.is_empty())
            .any(|runtime| !incoming_providers.contains(&runtime.provider))
    }

    /// Reports whether replacing a workspace with an authoritative full
    /// registration response would remove any currently published runtime.
    /// Callers use this before applying the response so task claims can be
    /// paused until executions tied to the retiring identities have drained.
    pub(crate) fn registration_demotion_required(
        &self,
        workspace_id: &str,
        incoming_runtime_ids: &BTreeSet<String>,
    ) -> bool {
        let state = self.state.read().unwrap();
        state.workspaces.get(workspace_id).is_some_and(|workspace| {
            workspace
                .runtime_ids
                .iter()
                .any(|runtime_id| !incoming_runtime_ids.contains(runtime_id))
        })
    }

    pub fn workspace_needs_runtime_recovery(&self, workspace_id: &str) -> bool {
        self.state
            .read()
            .unwrap()
            .workspaces
            .get(workspace_id)
            .is_some_and(|workspace| workspace.runtime_ids.is_empty())
    }
}

fn publish_runtime_set(state: &RegistryState, runtime_set: &RuntimeSet) {
    runtime_set.replace(state.runtimes.keys().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(id: &str, provider: &str) -> Runtime {
        Runtime {
            id: id.to_string(),
            provider: provider.to_string(),
            ..Runtime::default()
        }
    }

    #[test]
    fn registration_replacement_publishes_one_authoritative_set() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let first = registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("r-2", "claude"), runtime("r-1", "codex")],
            )
            .unwrap();
        assert_eq!(first.added, vec!["r-1".to_string(), "r-2".to_string()]);
        assert!(first.dropped.is_empty());
        assert_eq!(
            published.snapshot(),
            vec!["r-1".to_string(), "r-2".to_string()]
        );
        assert_eq!(
            registry.provider_for_runtime("r-2").as_deref(),
            Some("claude")
        );

        let second = registry
            .apply_registration("ws-1", "One", vec![runtime("r-3", "codex")])
            .unwrap();
        assert_eq!(second.added, vec!["r-3".to_string()]);
        assert_eq!(second.dropped, vec!["r-1".to_string(), "r-2".to_string()]);
        assert_eq!(published.snapshot(), vec!["r-3".to_string()]);
    }

    #[test]
    fn runtime_gone_returns_owner_and_updates_health_and_control() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("r-1", "codex"), runtime("r-2", "claude")],
            )
            .unwrap();

        assert_eq!(registry.remove_runtime("r-1").as_deref(), Some("ws-1"));
        assert_eq!(published.snapshot(), vec!["r-2".to_string()]);
        assert_eq!(
            registry.health_workspaces()[0].runtimes,
            vec!["r-2".to_string()]
        );
    }

    #[test]
    fn cross_workspace_runtime_collision_is_rejected_atomically() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        registry
            .apply_registration("ws-1", "One", vec![runtime("r-1", "codex")])
            .unwrap();

        let error = registry
            .apply_registration("ws-2", "Two", vec![runtime("r-1", "claude")])
            .unwrap_err();
        assert!(error.to_string().contains("already owned"));
        assert_eq!(registry.workspace_ids(), vec!["ws-1".to_string()]);
        assert_eq!(
            registry.provider_for_runtime("r-1").as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn builtin_refresh_preserves_custom_profile_runtimes() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let mut profile = runtime("profile-runtime", "codex");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("old-builtin", "codex"), profile],
            )
            .unwrap();

        let delta = registry
            .apply_builtin_registration("ws-1", "One", vec![runtime("new-builtin", "claude")])
            .unwrap();
        assert_eq!(delta.added, vec!["new-builtin".to_string()]);
        assert_eq!(delta.dropped, vec!["old-builtin".to_string()]);
        assert_eq!(
            published.snapshot(),
            vec!["new-builtin".to_string(), "profile-runtime".to_string()]
        );
        assert_eq!(
            registry.provider_for_runtime("profile-runtime").as_deref(),
            Some("codex")
        );
        assert_eq!(
            registry.execution_target_for_runtime("profile-runtime"),
            Some(RuntimeExecutionTarget {
                provider: "codex".to_string(),
                profile_id: "profile-1".to_string(),
            })
        );
    }

    #[test]
    fn empty_profile_refresh_converges_workspace_to_zero() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let mut profile = runtime("profile-runtime", "codex");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration("ws-1", "One", vec![profile])
            .unwrap();

        let delta = registry
            .apply_registration("ws-1", "One", Vec::new())
            .unwrap();

        assert!(delta.added.is_empty());
        assert_eq!(delta.dropped, vec!["profile-runtime".to_string()]);
        assert!(published.snapshot().is_empty());
        assert!(registry.workspace("ws-1").is_some());
    }

    #[test]
    fn empty_builtin_refresh_preserves_custom_profiles() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let mut profile = runtime("profile-runtime", "codex");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("builtin-runtime", "claude"), profile],
            )
            .unwrap();

        let delta = registry
            .apply_builtin_registration("ws-1", "One", Vec::new())
            .unwrap();

        assert_eq!(delta.dropped, vec!["builtin-runtime".to_string()]);
        assert_eq!(published.snapshot(), vec!["profile-runtime".to_string()]);
    }

    #[test]
    fn transient_probe_preserves_builtin_while_profiles_converge() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let mut profile = runtime("old-profile", "codex");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("builtin", "codex"), profile],
            )
            .unwrap();

        let delta = registry
            .apply_registration_guarded(
                "ws-1",
                "One",
                Vec::new(),
                &BTreeSet::from(["codex".to_string()]),
            )
            .unwrap();

        assert_eq!(delta.dropped, vec!["old-profile".to_string()]);
        assert!(delta.revived.ids.is_empty());
        assert_eq!(published.snapshot(), vec!["builtin".to_string()]);
    }

    #[test]
    fn demotion_hold_rejects_late_response_and_generation_safe_recovery() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let mut profile = runtime("profile-runtime", "codex");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("old-builtin", "codex"), profile],
            )
            .unwrap();
        let reason = crate::client::RuntimeOfflineReason {
            code: crate::client::RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE.to_string(),
            detail: "exec format".to_string(),
            repair: None,
        };
        let causes = BTreeMap::from([(
            "codex".to_string(),
            RuntimeVerdict {
                reason: "exec format".to_string(),
                offline: Some(reason.clone()),
            },
        )]);

        let partition = registry.demote_builtins(&causes);
        assert_eq!(partition.demoted_ids, vec!["old-builtin".to_string()]);
        assert_eq!(published.snapshot(), vec!["profile-runtime".to_string()]);

        let late = registry
            .apply_builtin_registration_guarded(
                "ws-1",
                "One",
                vec![runtime("revived", "codex")],
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(late.revived.ids, vec!["revived".to_string()]);
        assert_eq!(late.revived.reasons["revived"], reason);
        assert_eq!(published.snapshot(), vec!["profile-runtime".to_string()]);

        registry.clear_recovered_providers(&BTreeSet::from(["codex".to_string()]), 0);
        let stale_probe = registry
            .apply_builtin_registration_guarded(
                "ws-1",
                "One",
                vec![runtime("still-rejected", "codex")],
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(stale_probe.revived.ids, vec!["still-rejected".to_string()]);

        registry.clear_recovered_providers(&BTreeSet::from(["codex".to_string()]), 1);
        let recovered = registry
            .apply_builtin_registration_guarded(
                "ws-1",
                "One",
                vec![runtime("new-builtin", "codex")],
                &BTreeSet::new(),
            )
            .unwrap();
        assert!(recovered.revived.ids.is_empty());
        assert_eq!(
            published.snapshot(),
            vec!["new-builtin".to_string(), "profile-runtime".to_string()]
        );
    }

    #[test]
    fn builtin_demotion_detection_ignores_custom_profiles() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(published);
        let mut profile = runtime("profile-runtime", "custom-family");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![runtime("builtin-runtime", "codex"), profile],
            )
            .unwrap();

        assert!(!registry.builtin_demotion_required("ws-1", &BTreeSet::from(["codex".to_string()])));
        assert!(registry.builtin_demotion_required("ws-1", &BTreeSet::new()));
    }

    #[test]
    fn full_registration_demotion_detection_tracks_runtime_ids() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(published);
        registry
            .apply_registration(
                "ws-1",
                "One",
                vec![
                    runtime("runtime-1", "codex"),
                    runtime("runtime-2", "claude"),
                ],
            )
            .unwrap();

        assert!(!registry.registration_demotion_required(
            "ws-1",
            &BTreeSet::from(["runtime-1".to_string(), "runtime-2".to_string()]),
        ));
        assert!(registry.registration_demotion_required(
            "ws-1",
            &BTreeSet::from(["runtime-2".to_string(), "runtime-3".to_string()]),
        ));
        assert!(registry.registration_demotion_required("ws-1", &BTreeSet::new()));
        assert!(!registry.registration_demotion_required("unknown", &BTreeSet::new()));
    }
}
