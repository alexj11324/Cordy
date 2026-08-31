//! The daemon's local health endpoint wire types, repository-checkout
//! authorization, and bearer-token registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `HealthResponse`: returned by the daemon's local health endpoint.
///
/// OS lets the desktop app detect a foreign-OS daemon (#3916); Profile is
/// deliberately NOT omitted because the
/// empty string is a real answer ("I am the default profile's daemon") that
/// must stay distinguishable from a pre-#6694 daemon (#6694); SkippedAgents is
/// what made GH #6077 actionable (PB-5439).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthResponse {
    pub status: String,
    pub pid: i32,
    pub os: String,
    pub uptime: String,
    pub profile: String,
    #[serde(rename = "daemon_id")]
    pub daemon_id: String,
    #[serde(rename = "device_name")]
    pub device_name: String,
    #[serde(rename = "server_url")]
    pub server_url: String,
    #[serde(rename = "cli_version")]
    pub cli_version: String,
    #[serde(
        rename = "launched_by",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub launched_by: String,
    #[serde(rename = "active_task_count")]
    pub active_task_count: i64,
    #[serde(rename = "running_task_count")]
    pub running_task_count: i64,
    #[serde(rename = "resource_wait_task_count")]
    pub resource_wait_task_count: i64,
    #[serde(
        rename = "repo_maintenance_active",
        default,
        skip_serializing_if = "is_zero"
    )]
    pub repo_maintenance_active: i64,
    #[serde(
        rename = "repo_checkout_waiters",
        default,
        skip_serializing_if = "is_zero"
    )]
    pub repo_checkout_waiters: i64,
    #[serde(default)]
    pub agents: Vec<String>,
    /// Maps a discovered provider to why registration dropped it. Omitted when
    /// empty so older consumers see no change (PB-5439).
    #[serde(
        rename = "skipped_agents",
        default,
        skip_serializing_if = "std::collections::HashMap::is_empty"
    )]
    pub skipped_agents: HashMap<String, String>,
    /// Why a confirmed patchbay version change hasn't restarted yet. Diagnostic
    /// only.
    #[serde(
        rename = "reload_pending_reason",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub reload_pending_reason: String,
    #[serde(default)]
    pub workspaces: Vec<HealthWorkspace>,
}

/// Machine-level provider diagnostics from the latest successful discovery
/// round. The registration owner replaces this snapshot atomically so the
/// health endpoint never combines agents from one probe with skip reasons
/// from another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentHealthSnapshot {
    pub agents: Vec<String>,
    pub skipped_agents: HashMap<String, String>,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthWorkspace {
    pub id: String,
    pub runtimes: Vec<String>,
}

/// `repoCheckoutRequest`: body of POST /repo/checkout.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RepoCheckoutRequest {
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(default)]
    pub workdir: String,
    #[serde(default)]
    pub r#ref: String,
    #[serde(default, rename = "agent_name")]
    pub agent_name: String,
    #[serde(default, rename = "task_id")]
    pub task_id: String,
    #[serde(default, rename = "checkout_mode")]
    pub checkout_mode: String,
    /// Sent by clients that understand 503 + Retry-After; older clients omit
    /// it and keep unbounded lock-wait behavior.
    #[serde(default, rename = "retry_busy")]
    pub retry_busy: bool,
}

/// One daemon-owned checkout that must be refreshed from the checkout path at
/// task termination. The initial branch/workspace facts are only a durable
/// execution hint; the terminal head is read from `path` before discovery is
/// queued.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoCheckoutProvenance {
    pub path: String,
    pub repo_identity: String,
    pub branch_name: String,
}

/// `activeRepoCheckoutTask`.
#[derive(Clone, Default)]
pub struct ActiveRepoCheckoutTask {
    pub workspace_id: String,
    pub task_id: String,
    /// Issue associated with the execution. Keeping this identity in the
    /// daemon-owned capability prevents a PR response from guessing an issue
    /// from a branch or title.
    pub issue_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub work_dir: String,
    /// Server-issued `mdt_` credential. It is used only by the daemon when
    /// reporting checkout provenance and is never copied into the agent env.
    pub execution_daemon_token: String,
    pub(crate) checkouts: std::sync::Arc<std::sync::Mutex<Vec<RepoCheckoutProvenance>>>,
}

impl std::fmt::Debug for ActiveRepoCheckoutTask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveRepoCheckoutTask")
            .field("workspace_id", &self.workspace_id)
            .field("task_id", &self.task_id)
            .field("issue_id", &self.issue_id)
            .field("agent_id", &self.agent_id)
            .field("agent_name", &self.agent_name)
            .field("work_dir", &self.work_dir)
            .field(
                "has_execution_daemon_token",
                &!self.execution_daemon_token.is_empty(),
            )
            .field("checkout_count", &self.checkouts.lock().unwrap().len())
            .finish()
    }
}

/// `REPO_CHECKOUT_LOCK_WAIT_TIMEOUT` etc. (health.go:179–184).
pub(crate) const REPO_CHECKOUT_LOCK_WAIT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

/// The registry half of Go's `d.repoCheckoutTasks` map (health.go:111–153):
/// binds checkout identity to the active task via a per-task bearer token. The
/// token prevents unauthenticated localhost callers from choosing another
/// task's identity or workdir; it is NOT an OS-user isolation boundary.
#[derive(Default)]
pub struct RepoCheckoutRegistry {
    tasks: std::sync::Mutex<HashMap<String, ActiveRepoCheckoutTask>>,
}

impl RepoCheckoutRegistry {
    /// `registerActiveRepoCheckoutTask`.
    pub fn register(&self, token: &str, task: ActiveRepoCheckoutTask) {
        self.tasks.lock().unwrap().insert(token.to_string(), task);
    }

    /// `clearActiveRepoCheckoutTask`.
    pub fn clear(&self, token: &str) {
        self.tasks.lock().unwrap().remove(token);
    }

    /// Ownership-safe provider execution seam. Dropping the returned guard
    /// always revokes the task credential, including cancellation and unwind
    /// paths around provider execution.
    pub fn register_owned(
        self: &std::sync::Arc<Self>,
        token: impl Into<String>,
        task: ActiveRepoCheckoutTask,
    ) -> RepoCheckoutTaskGuard {
        let token = token.into();
        self.register(&token, task);
        RepoCheckoutTaskGuard {
            registry: std::sync::Arc::clone(self),
            token,
        }
    }

    /// `activeRepoCheckoutTask(r)`: resolves the Authorization header's bearer
    /// token against the registry.
    pub(crate) fn resolve(&self, authorization_header: &str) -> Option<ActiveRepoCheckoutTask> {
        let header = authorization_header.trim();
        let token = header.strip_prefix("Bearer ")?.trim();
        if token.is_empty() {
            return None;
        }
        self.tasks.lock().unwrap().get(token).cloned()
    }

    /// Records a checkout while its owning task credential is active. The
    /// checkout path is retained in memory until terminal provenance has been
    /// refreshed, so a task that checks out multiple repositories does not
    /// collapse to whichever checkout happened to be last.
    pub(crate) fn record_checkout(&self, task_id: &str, checkout: RepoCheckoutProvenance) -> bool {
        let task = self
            .tasks
            .lock()
            .unwrap()
            .values()
            .find(|task| task.task_id == task_id)
            .cloned();
        let Some(task) = task else {
            return false;
        };
        let mut checkouts = task.checkouts.lock().unwrap();
        if !checkouts.iter().any(|existing| {
            existing.path == checkout.path
                && existing.repo_identity == checkout.repo_identity
                && existing.branch_name == checkout.branch_name
        }) {
            checkouts.push(checkout);
        }
        true
    }

    /// Returns the exact checkout paths owned by the active task. Callers must
    /// invoke this before dropping the task guard, which revokes the registry
    /// entry and makes late localhost checkout calls unauthorized.
    pub(crate) fn checkouts_for_task(&self, task_id: &str) -> Vec<RepoCheckoutProvenance> {
        self.tasks
            .lock()
            .unwrap()
            .values()
            .find(|task| task.task_id == task_id)
            .map(|task| task.checkouts.lock().unwrap().clone())
            .unwrap_or_default()
    }
}

pub struct RepoCheckoutTaskGuard {
    registry: std::sync::Arc<RepoCheckoutRegistry>,
    token: String,
}

impl Drop for RepoCheckoutTaskGuard {
    fn drop(&mut self) {
        self.registry.clear(&self.token);
    }
}

/// `authorizeRepoCheckoutWorkDir` (health.go:155): authorizes `requested`
/// inside `active_root` after resolving both through symlinks; rejects
/// anything outside the active task workdir root.
pub(crate) fn authorize_repo_checkout_workdir(
    active_root: &str,
    requested: &str,
) -> anyhow::Result<PathBuf> {
    let abs = |p: &str| -> anyhow::Result<PathBuf> {
        let path = Path::new(p);
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .map_err(|err| anyhow::Error::from(err).context("abs"))
        }
    };
    // filepath.EvalSymlinks fails on non-existent paths, exactly like
    // fs.realpath; canonicalize additionally absolutizes, so abs() first is
    // redundant but harmless and keeps error ordering identical.
    let root = abs(active_root)?;
    let root =
        std::fs::canonicalize(&root).map_err(|err| anyhow::Error::from(err).context("root"))?;
    let workdir = abs(requested)?;
    let workdir = std::fs::canonicalize(&workdir)
        .map_err(|err| anyhow::Error::from(err).context("workdir"))?;

    let rel = match workdir.strip_prefix(&root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
        _ => {
            // Either outside the root entirely, or the root itself (Go:
            // Rel returns "." which IsLocal accepts... but "." as workdir is
            // the root itself; Go's IsLocal(".") == true). Preserve Go's
            // acceptance of the root by allowing empty rel.
            if workdir == root {
                PathBuf::from(".")
            } else {
                anyhow::bail!("workdir is outside the active task workdir");
            }
        }
    };
    if rel.components().any(|c| c.as_os_str() == "..") {
        anyhow::bail!("workdir is outside the active task workdir");
    }
    Ok(workdir)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn owned_checkout_credential_is_revoked_on_drop() {
        let registry = Arc::new(RepoCheckoutRegistry::default());
        let guard = registry.register_owned(
            "task-token",
            ActiveRepoCheckoutTask {
                task_id: "task-1".to_string(),
                ..ActiveRepoCheckoutTask::default()
            },
        );
        assert_eq!(
            registry
                .resolve("Bearer task-token")
                .map(|task| task.task_id)
                .as_deref(),
            Some("task-1")
        );

        drop(guard);
        assert!(registry.resolve("Bearer task-token").is_none());
    }
}
