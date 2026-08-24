//! Authoritative daemon workspace/runtime registration state.
//!
//! Every registration response is applied under one lock and publishes the
//! resulting complete [`RuntimeSet`] before releasing ownership. Runtime
//! lookup, runtime-gone recovery, health, and control transport therefore
//! cannot observe independently maintained identity maps.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use crate::health::HealthWorkspace;
use crate::runtime_set::RuntimeSet;
use crate::types::Runtime;

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
}

#[derive(Default)]
struct RegistryState {
    workspaces: BTreeMap<String, WorkspaceRuntimeState>,
    runtimes: BTreeMap<String, Runtime>,
    runtime_workspaces: BTreeMap<String, String>,
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
        let workspace_id = workspace_id.into();
        anyhow::ensure!(!workspace_id.is_empty(), "workspace id is required");

        let mut incoming_ids = BTreeSet::new();
        for runtime in &runtimes {
            anyhow::ensure!(!runtime.id.is_empty(), "registered runtime id is required");
            anyhow::ensure!(
                !runtime.provider.is_empty(),
                "registered runtime {} has no provider",
                runtime.id
            );
            anyhow::ensure!(
                incoming_ids.insert(runtime.id.clone()),
                "registration returned duplicate runtime id {}",
                runtime.id
            );
        }

        let mut state = self.state.write().unwrap();
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
        Ok(RegistrationDelta { added, dropped })
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
        self.apply_registration(workspace_id, workspace_name, combined)
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
}
