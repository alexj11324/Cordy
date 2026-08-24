//! Port of `server/internal/daemon/health.go` (lines 19–184, 179–406) — the
//! daemon's local health endpoint surface.
//!
//! Symbol map (Go → Rust):
//! - `HealthResponse` / `healthWorkspace` → [`HealthResponse`] /
//!   [`HealthWorkspace`]
//! - `repoCheckoutRequest` → [`RepoCheckoutRequest`]
//! - `activeRepoCheckoutTask` → [`ActiveRepoCheckoutTask`]
//! - `registerActiveRepoCheckoutTask` / `clearActiveRepoCheckoutTask` /
//!   `activeRepoCheckoutTask(r)` → [`RepoCheckoutRegistry`] methods
//! - `authorizeRepoCheckoutWorkDir` → [`authorize_repo_checkout_workdir`]
//! - constants → [`REPO_CHECKOUT_*`]
//!
//! S9-integration: `listenHealth`, `healthHandler`, `shutdownHandler`,
//! `serveHealth` and `repoCheckoutHandler` are Daemon methods (lane B) — they
//! read d.cfg/d.mu/d.ready/d.repoCache and own the HTTP mux. They land with
//! daemon.go core; this module carries everything those handlers need that is
//! Daemon-independent (wire types + workdir authorization + the bearer-token
//! registry), so lane B only wires them.

// S9-integration: consumed by daemon.go core (lane B) health server wiring; silence dead-code until wired.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `HealthResponse`: returned by the daemon's local health endpoint.
///
/// Field-level notes preserved from Go: OS lets the desktop app detect a
/// foreign-OS daemon (#3916); Profile is deliberately NOT omitempty because the
/// empty string is a real answer ("I am the default profile's daemon") that
/// must stay distinguishable from a pre-#6694 daemon (#6694); SkippedAgents is
/// what made GH #6077 actionable (MUL-5439).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// empty so older consumers see no change (MUL-5439).
    #[serde(
        rename = "skipped_agents",
        default,
        skip_serializing_if = "std::collections::HashMap::is_empty"
    )]
    pub skipped_agents: HashMap<String, String>,
    /// Why a confirmed cordy version change hasn't restarted yet. Diagnostic
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

/// `activeRepoCheckoutTask`.
#[derive(Debug, Clone, Default)]
pub struct ActiveRepoCheckoutTask {
    pub workspace_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub work_dir: String,
}

/// `REPO_CHECKOUT_LOCK_WAIT_TIMEOUT` etc. (health.go:179–184).
pub(crate) const REPO_CHECKOUT_LOCK_WAIT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);
pub(crate) const REPO_CHECKOUT_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(2);
pub(crate) const REPO_CHECKOUT_RETRY_HEADER: &str = "X-Cordy-Retryable";
pub(crate) const REPO_CHECKOUT_RETRY_VALUE_BUSY: &str = "repo-busy";

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
