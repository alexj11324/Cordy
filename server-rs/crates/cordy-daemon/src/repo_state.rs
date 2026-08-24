//! Daemon-owned workspace and task repository authorization state.
//!
//! Server registration/refresh responses replace workspace bindings while
//! claimed-task repositories are unioned separately and carry task-scoped
//! default refs. Local checkout authorization and GC liveness read this same
//! state; provider implementations never maintain a parallel allowlist.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use crate::types::RepoData;

#[derive(Default)]
struct WorkspaceRepos {
    allowed: BTreeSet<String>,
    task_urls: BTreeSet<String>,
    task_refs: BTreeMap<String, BTreeMap<String, String>>,
    settings: Option<Value>,
    last_sync_error: String,
    refresh_lock: Arc<AsyncMutex<()>>,
}

#[derive(Default)]
pub struct DaemonRepoState {
    workspaces: RwLock<BTreeMap<String, WorkspaceRepos>>,
}

impl DaemonRepoState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_workspace(
        &self,
        workspace_id: &str,
        repos: &[RepoData],
        settings: Option<Value>,
    ) {
        let allowed = repos
            .iter()
            .map(|repo| repo.url.trim())
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect();
        let mut workspaces = self.workspaces.write().unwrap();
        let workspace = workspaces.entry(workspace_id.to_string()).or_default();
        workspace.allowed = allowed;
        workspace.settings = settings;
        workspace.last_sync_error.clear();
    }

    pub fn remove_workspace(&self, workspace_id: &str) {
        self.workspaces.write().unwrap().remove(workspace_id);
    }

    /// Registers repositories surfaced only by one claimed task and returns
    /// the normalized candidates, for cache synchronization by the caller.
    pub fn register_task_repos(
        &self,
        workspace_id: &str,
        task_id: &str,
        repos: &[RepoData],
    ) -> Vec<String> {
        let mut workspaces = self.workspaces.write().unwrap();
        let Some(workspace) = workspaces.get_mut(workspace_id) else {
            return Vec::new();
        };
        let mut candidates = BTreeSet::new();
        for repo in repos {
            let url = repo.url.trim();
            if url.is_empty() {
                continue;
            }
            workspace.task_urls.insert(url.to_string());
            if !task_id.is_empty() {
                workspace
                    .task_refs
                    .entry(task_id.to_string())
                    .or_default()
                    .entry(url.to_string())
                    .or_insert_with(|| repo.ref_.trim().to_string());
            }
            candidates.insert(url.to_string());
        }
        candidates.into_iter().collect()
    }

    pub fn clear_task_refs(&self, workspace_id: &str, task_id: &str) {
        if let Some(workspace) = self.workspaces.write().unwrap().get_mut(workspace_id) {
            workspace.task_refs.remove(task_id);
        }
    }

    pub fn is_allowed(&self, workspace_id: &str, url: &str) -> bool {
        self.workspaces
            .read()
            .unwrap()
            .get(workspace_id)
            .is_some_and(|workspace| {
                workspace.allowed.contains(url) || workspace.task_urls.contains(url)
            })
    }

    pub fn task_default_ref(&self, workspace_id: &str, task_id: &str, url: &str) -> String {
        self.workspaces
            .read()
            .unwrap()
            .get(workspace_id)
            .and_then(|workspace| workspace.task_refs.get(task_id))
            .and_then(|refs| refs.get(url))
            .cloned()
            .unwrap_or_default()
    }

    pub fn co_authored_by_enabled(&self, workspace_id: &str) -> bool {
        #[derive(Deserialize)]
        struct Settings {
            github_enabled: Option<bool>,
            co_authored_by_enabled: Option<bool>,
        }

        let workspaces = self.workspaces.read().unwrap();
        let Some(settings) = workspaces
            .get(workspace_id)
            .and_then(|workspace| workspace.settings.as_ref())
        else {
            return true;
        };
        let Ok(settings) = serde_json::from_value::<Settings>(settings.clone()) else {
            return true;
        };
        if settings.github_enabled == Some(false) {
            return false;
        }
        settings.co_authored_by_enabled.unwrap_or(true)
    }

    pub fn set_sync_error(&self, workspace_id: &str, error: String) {
        if let Some(workspace) = self.workspaces.write().unwrap().get_mut(workspace_id) {
            workspace.last_sync_error = error;
        }
    }

    pub fn last_sync_error(&self, workspace_id: &str) -> String {
        self.workspaces
            .read()
            .unwrap()
            .get(workspace_id)
            .map(|workspace| workspace.last_sync_error.clone())
            .unwrap_or_default()
    }

    pub fn refresh_lock(&self, workspace_id: &str) -> Option<Arc<AsyncMutex<()>>> {
        self.workspaces
            .read()
            .unwrap()
            .get(workspace_id)
            .map(|workspace| Arc::clone(&workspace.refresh_lock))
    }

    pub fn all_urls(&self) -> Vec<(String, String)> {
        let workspaces = self.workspaces.read().unwrap();
        let mut urls = Vec::new();
        for (workspace_id, workspace) in workspaces.iter() {
            for url in workspace.allowed.union(&workspace.task_urls) {
                urls.push((workspace_id.clone(), url.clone()));
            }
        }
        urls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_replace_preserves_task_repos_and_refs() {
        let state = DaemonRepoState::new();
        state.replace_workspace(
            "ws-1",
            &[RepoData {
                url: "https://example.test/workspace.git".into(),
                ..RepoData::default()
            }],
            None,
        );
        state.register_task_repos(
            "ws-1",
            "task-1",
            &[RepoData {
                url: "https://example.test/project.git".into(),
                ref_: "release".into(),
                ..RepoData::default()
            }],
        );
        state.replace_workspace("ws-1", &[], None);

        assert!(!state.is_allowed("ws-1", "https://example.test/workspace.git"));
        assert!(state.is_allowed("ws-1", "https://example.test/project.git"));
        assert_eq!(
            state.task_default_ref("ws-1", "task-1", "https://example.test/project.git"),
            "release"
        );
    }

    #[test]
    fn coauthor_policy_defaults_open_and_honors_master_switch() {
        let state = DaemonRepoState::new();
        state.replace_workspace("ws-1", &[], None);
        assert!(state.co_authored_by_enabled("ws-1"));

        state.replace_workspace(
            "ws-1",
            &[],
            Some(serde_json::json!({
                "github_enabled": false,
                "co_authored_by_enabled": true
            })),
        );
        assert!(!state.co_authored_by_enabled("ws-1"));
    }
}
