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
use crate::types::{Runtime, RuntimeExecutionTarget};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRuntimeState {
    pub id: String,
    pub name: String,
    pub runtime_ids: Vec<String>,
    /// Versions carried by the last accepted register payload for each
    /// built-in provider. The refresh path compares this per-workspace record
    /// with the latest machine probe so one failed workspace does not hide
    /// behind a global payload cache.
    pub builtin_versions: BTreeMap<String, String>,
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
                builtin_versions: BTreeMap::new(),
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

    /// Returns whether a workspace is missing one of the built-in providers
    /// in `payload` or has acknowledged a different version. The payload is
    /// machine-level, but the acknowledgement is deliberately workspace
    /// scoped because registration requests can fail independently.
    pub fn workspace_needs_builtin_refresh(
        &self,
        workspace_id: &str,
        payload: &[BTreeMap<String, String>],
    ) -> bool {
        let expected = builtin_versions_from_payload(payload);
        if expected.is_empty() {
            return false;
        }
        let state = self.state.read().unwrap();
        let Some(workspace) = state.workspaces.get(workspace_id) else {
            return false;
        };
        for (provider, version) in expected {
            let registered = workspace.runtime_ids.iter().any(|runtime_id| {
                state.runtimes.get(runtime_id).is_some_and(|runtime| {
                    runtime.profile_id.is_empty() && runtime.provider == provider
                })
            });
            if !registered || workspace.builtin_versions.get(&provider) != Some(&version) {
                return true;
            }
        }
        false
    }

    /// Returns whether a workspace is missing any currently available
    /// built-in provider, without requiring a version probe. This is the
    /// cheap discovery half; version lag is handled by the slower refresh.
    pub fn workspace_missing_builtin_provider(
        &self,
        workspace_id: &str,
        providers: &BTreeSet<String>,
    ) -> bool {
        if providers.is_empty() {
            return false;
        }
        let state = self.state.read().unwrap();
        let Some(workspace) = state.workspaces.get(workspace_id) else {
            return false;
        };
        providers.iter().any(|provider| {
            !workspace.runtime_ids.iter().any(|runtime_id| {
                state.runtimes.get(runtime_id).is_some_and(|runtime| {
                    runtime.profile_id.is_empty() && runtime.provider == *provider
                })
            })
        })
    }

    /// Records only the built-in entries carried by a successful register
    /// call. Providers absent from the payload are left untouched: a failed
    /// probe is not an acknowledgement that the server changed that row.
    pub fn record_builtin_versions(
        &self,
        workspace_id: &str,
        payload: &[BTreeMap<String, String>],
    ) {
        let expected = builtin_versions_from_payload(payload);
        if expected.is_empty() {
            return;
        }
        let mut state = self.state.write().unwrap();
        let Some(workspace) = state.workspaces.get_mut(workspace_id) else {
            return;
        };
        workspace.builtin_versions.extend(expected);
    }

    /// Merges a built-in-only register response into the accepted workspace
    /// state. Omitted providers remain tracked, matching Go's discovery and
    /// version-refresh paths: a transient probe failure must not tear down a
    /// working runtime or its heartbeat. A rotated runtime ID replaces only
    /// that provider and returns the old ID for best-effort server cleanup.
    pub fn merge_builtin_registration(
        &self,
        workspace_id: &str,
        _workspace_name: &str,
        builtins: Vec<Runtime>,
    ) -> anyhow::Result<RegistrationDelta> {
        anyhow::ensure!(
            builtins.iter().all(|runtime| runtime.profile_id.is_empty()),
            "built-in registration returned a custom profile runtime"
        );

        let mut incoming_ids = BTreeSet::new();
        for runtime in &builtins {
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
        let previous_ids = state
            .workspaces
            .get(workspace_id)
            .ok_or_else(|| anyhow::anyhow!("workspace {workspace_id} is not tracked"))?
            .runtime_ids
            .clone();
        for runtime_id in &incoming_ids {
            if let Some(owner) = state.runtime_workspaces.get(runtime_id) {
                anyhow::ensure!(
                    owner == workspace_id,
                    "runtime {runtime_id} is already owned by workspace {owner}"
                );
            }
        }

        let mut existing_by_provider = BTreeMap::new();
        let mut kept = previous_ids.clone();
        let mut present: BTreeSet<String> = previous_ids.iter().cloned().collect();
        for runtime_id in &previous_ids {
            if let Some(runtime) = state.runtimes.get(runtime_id) {
                if runtime.profile_id.is_empty() {
                    existing_by_provider.insert(runtime.provider.clone(), runtime_id.clone());
                }
            }
        }

        let mut added = Vec::new();
        let mut dropped = Vec::new();
        for runtime in builtins {
            let mut replaced = false;
            if let Some(old_id) = existing_by_provider.get(&runtime.provider).cloned() {
                if old_id != runtime.id {
                    if let Some(slot) = kept.iter_mut().find(|id| **id == old_id) {
                        *slot = runtime.id.clone();
                        replaced = true;
                    }
                    state.runtimes.remove(&old_id);
                    state.runtime_workspaces.remove(&old_id);
                    present.remove(&old_id);
                    dropped.push(old_id);
                }
            }

            state
                .runtime_workspaces
                .insert(runtime.id.clone(), workspace_id.to_string());
            state.runtimes.insert(runtime.id.clone(), runtime.clone());
            if replaced {
                present.insert(runtime.id.clone());
                added.push(runtime.id.clone());
            } else if present.insert(runtime.id.clone()) {
                kept.push(runtime.id.clone());
                added.push(runtime.id.clone());
            }
            existing_by_provider.insert(runtime.provider, runtime.id);
        }

        state
            .workspaces
            .get_mut(workspace_id)
            .expect("workspace checked above")
            .runtime_ids = kept;
        publish_runtime_set(&state, &self.runtime_set);
        Ok(RegistrationDelta { added, dropped })
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

    /// Resolves the complete launch identity from the same authoritative row
    /// that accepted the task's runtime ID. Keeping `profile_id` attached is
    /// required for custom runtime command overrides and fixed arguments.
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

    pub fn workspace_needs_runtime_recovery(&self, workspace_id: &str) -> bool {
        self.state
            .read()
            .unwrap()
            .workspaces
            .get(workspace_id)
            .is_some_and(|workspace| workspace.runtime_ids.is_empty())
    }
}

fn builtin_versions_from_payload(payload: &[BTreeMap<String, String>]) -> BTreeMap<String, String> {
    payload
        .iter()
        .filter(|runtime| {
            runtime
                .get("profile_id")
                .is_none_or(|profile_id| profile_id.is_empty())
        })
        .filter_map(|runtime| {
            let provider = runtime.get("type")?.clone();
            let version = runtime.get("version").cloned().unwrap_or_default();
            Some((provider, version))
        })
        .collect()
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
            registry.execution_target_for_runtime("r-2"),
            Some(RuntimeExecutionTarget {
                provider: "claude".to_string(),
                profile_id: String::new(),
            })
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
            registry.execution_target_for_runtime("r-1"),
            Some(RuntimeExecutionTarget {
                provider: "codex".to_string(),
                profile_id: String::new(),
            })
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
            registry.execution_target_for_runtime("profile-runtime"),
            Some(RuntimeExecutionTarget {
                provider: "codex".to_string(),
                profile_id: "profile-1".to_string(),
            })
        );
        assert_eq!(
            registry.execution_target_for_runtime("new-builtin"),
            Some(RuntimeExecutionTarget {
                provider: "claude".to_string(),
                profile_id: String::new(),
            })
        );
    }

    #[test]
    fn cheap_builtin_missing_scan_ignores_custom_profiles() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        let mut profile = runtime("profile-runtime", "claude");
        profile.profile_id = "profile-1".to_string();
        registry
            .apply_registration("ws-1", "One", vec![profile])
            .unwrap();

        assert!(registry
            .workspace_missing_builtin_provider("ws-1", &BTreeSet::from(["claude".to_string()])));
        assert!(!registry.workspace_missing_builtin_provider("ws-1", &BTreeSet::new()));
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

    fn builtin_payload(provider: &str, version: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("status".to_string(), "online".to_string()),
            ("type".to_string(), provider.to_string()),
            ("version".to_string(), version.to_string()),
        ])
    }

    #[test]
    fn builtin_refresh_merges_omitted_provider_and_tracks_ack_per_workspace() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        registry
            .apply_registration("ws-1", "One", vec![runtime("codex-1", "codex")])
            .unwrap();
        registry
            .apply_registration("ws-2", "Two", vec![runtime("codex-2", "codex")])
            .unwrap();

        let old_payload = vec![builtin_payload("codex", "1.0.0")];
        registry.record_builtin_versions("ws-1", &old_payload);
        registry.record_builtin_versions("ws-2", &old_payload);
        assert!(!registry.workspace_needs_builtin_refresh("ws-1", &old_payload));

        let upgraded_payload = vec![builtin_payload("codex", "2.0.0")];
        assert!(registry.workspace_needs_builtin_refresh("ws-1", &upgraded_payload));
        assert!(registry.workspace_needs_builtin_refresh("ws-2", &upgraded_payload));

        // A response that only carries the newly detected provider must not
        // evict the already-running provider omitted by a transient probe.
        let delta = registry
            .merge_builtin_registration("ws-1", "One", vec![runtime("claude-1", "claude")])
            .unwrap();
        assert_eq!(delta.added, vec!["claude-1".to_string()]);
        assert!(delta.dropped.is_empty());
        assert_eq!(
            published.snapshot(),
            vec![
                "claude-1".to_string(),
                "codex-1".to_string(),
                "codex-2".to_string()
            ]
        );
        assert!(registry
            .workspace("ws-1")
            .unwrap()
            .runtime_ids
            .contains(&"codex-1".to_string()));

        registry.record_builtin_versions("ws-1", &upgraded_payload);
        assert!(!registry.workspace_needs_builtin_refresh("ws-1", &upgraded_payload));
        assert!(registry.workspace_needs_builtin_refresh("ws-2", &upgraded_payload));
    }

    #[test]
    fn builtin_refresh_rotation_replaces_id_without_duplicate_heartbeats() {
        let published = Arc::new(RuntimeSet::new());
        let registry = RuntimeRegistry::new(Arc::clone(&published));
        registry
            .apply_registration("ws-1", "One", vec![runtime("old", "codex")])
            .unwrap();

        let delta = registry
            .merge_builtin_registration("ws-1", "One", vec![runtime("new", "codex")])
            .unwrap();
        assert_eq!(delta.added, vec!["new".to_string()]);
        assert_eq!(delta.dropped, vec!["old".to_string()]);
        assert_eq!(
            registry.workspace("ws-1").unwrap().runtime_ids,
            vec!["new".to_string()]
        );
        assert_eq!(published.snapshot(), vec!["new".to_string()]);
    }
}
