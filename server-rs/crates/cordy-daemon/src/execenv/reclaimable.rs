//! Port of execenv/reclaimable.go.
//!
//! Symbol map:
//! - codexHomeDirName / codexSandboxBinDirName → CODEX_HOME_DIR_NAME /
//!   CODEX_SANDBOX_BIN_DIR_NAME consts
//! - ManagedReclaimableArtifactSubpaths        → managed_reclaimable_artifact_subpaths

use super::execenv::join_path;

pub const CODEX_HOME_DIR_NAME: &str = "codex-home";
pub const CODEX_SANDBOX_BIN_DIR_NAME: &str = ".sandbox-bin";

/// ManagedReclaimableArtifactSubpaths returns daemon-owned, regenerable
/// directories inside a task env root. Callers must match these as exact
/// relative paths rather than basenames: a repository may legitimately contain
/// a directory with the same leaf name.
pub fn managed_reclaimable_artifact_subpaths() -> Vec<String> {
    vec![join_path(&[
        CODEX_HOME_DIR_NAME,
        CODEX_SANDBOX_BIN_DIR_NAME,
    ])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_managed_reclaimable_artifact_subpaths() {
        let subpaths = managed_reclaimable_artifact_subpaths();
        assert_eq!(subpaths, vec!["codex-home/.sandbox-bin".to_string()]);
    }
}
