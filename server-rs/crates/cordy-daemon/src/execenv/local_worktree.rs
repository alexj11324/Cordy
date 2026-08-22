//! TEMPORARY S9-integration stand-in for lane E1b's local_worktree port.
//! E1b replaces this file with the full local_worktree.go port.

/// `LocalWorktreeParams` (local_worktree.go:72–83).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LocalWorktreeParams {
    pub local_path: String,
    pub env_root: String,
    pub agent_name: String,
    pub task_id: String,
}

/// `LocalWorktree` (local_worktree.go:87+).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LocalWorktree {
    pub git_root: String,
    pub path: String,
    pub work_dir: String,
    pub branch: String,
    pub base_commit: String,
    pub dirty_base_captured: bool,
}

/// `PrepareLocalWorktree` — unimplemented stand-in (lane E1b ports the body).
pub async fn prepare_local_worktree(_params: LocalWorktreeParams) -> anyhow::Result<LocalWorktree> {
    anyhow::bail!("local_worktree port pending (lane E1b)")
}

impl LocalWorktree {
    /// `Discard` — unimplemented stand-in (lane E1b ports the body).
    pub async fn discard(&self) -> anyhow::Result<()> {
        anyhow::bail!("local_worktree discard port pending (lane E1b)")
    }
}
