//! Local-directory task assignment and worktree handling.
//!
//! Validates local project resources, resolves execution modes, and serializes
//! in-place tasks with a cancellable per-path mutex. Worktree assignments do
//! not use the in-place path lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{anyhow, Context as _};

use crate::repocache::{normalize_lexically, Ctx};
use crate::types::{ProjectResourceData, Task};

pub(crate) const LOCAL_DIRECTORY_RESOURCE_TYPE: &str = "local_directory";

pub(crate) const LOCAL_DIRECTORY_MODE_IN_PLACE: &str = "in_place";
pub(crate) const LOCAL_DIRECTORY_MODE_WORKTREE: &str = "worktree";

/// Server-side reference shape for local-directory project resources.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LocalDirectoryRef {
    pub local_path: String,
    pub daemon_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub execution_mode: String,
}

/// The resolved view of a task's local-directory resource.
#[derive(Debug, Clone)]
pub struct LocalDirectoryAssignment {
    pub reference: LocalDirectoryRef,
    /// user-provided path, cleaned but not symlink-resolved.
    pub abs_path: String,
    /// canonical key for the path mutex.
    pub real_path: String,
}

impl LocalDirectoryAssignment {
    /// `UsesWorktree` (go:58–60): worktree tasks skip the per-path mutex —
    /// that is the whole point of the mode.
    pub fn uses_worktree(&self) -> bool {
        self.reference.execution_mode.trim() == LOCAL_DIRECTORY_MODE_WORKTREE
    }

    /// `ValidateExecutionMode` (go:71–85): an unrecognised mode fails the task
    /// rather than silently running in place, because execution_mode is how a
    /// user asks for ISOLATION — losing concurrency is a nuisance; ignoring a
    /// request to not touch someone's files is a broken promise.
    pub fn validate_execution_mode(&self) -> anyhow::Result<()> {
        match self.reference.execution_mode.trim() {
            "" | LOCAL_DIRECTORY_MODE_IN_PLACE | LOCAL_DIRECTORY_MODE_WORKTREE => Ok(()),
            other => Err(anyhow!(
                "local_directory: this daemon does not support execution_mode {:?} for {:?} \
                 (update the daemon, or set the resource's execution mode to {:?} or {:?}); \
                 refusing to run in place, since that would modify a directory the resource asked to isolate",
                other, self.abs_path, LOCAL_DIRECTORY_MODE_IN_PLACE, LOCAL_DIRECTORY_MODE_WORKTREE
            )),
        }
    }
}

/// `localDirectoryAssignmentForTask` (go:91–96): squad-leader tasks are
/// coordinators and never bind to the user's repo worktree.
pub(crate) fn local_directory_assignment_for_task(
    task: &Task,
    daemon_id: &str,
) -> anyhow::Result<Option<LocalDirectoryAssignment>> {
    if task.is_leader_task {
        return Ok(None);
    }
    find_local_directory_assignment(&task.project_resources, daemon_id)
}

/// Scans the task's project resources for one of type local_directory whose
/// daemon_id matches this daemon (go:111–157). More than one match is a
/// server-side invariant violation — refuse to guess.
pub(crate) fn find_local_directory_assignment(
    resources: &[ProjectResourceData],
    daemon_id: &str,
) -> anyhow::Result<Option<LocalDirectoryAssignment>> {
    let mut matched: Option<LocalDirectoryAssignment> = None;
    for resource in resources {
        if resource.resource_type != LOCAL_DIRECTORY_RESOURCE_TYPE {
            continue;
        }
        let mut reference: LocalDirectoryRef =
            serde_json::from_value(resource.resource_ref.clone())
                .map_err(|err| anyhow!("local_directory: parse resource_ref: {err}"))?;
        reference.daemon_id = reference.daemon_id.trim().to_string();
        if reference.daemon_id.is_empty() {
            return Err(anyhow!("local_directory: resource_ref missing daemon_id"));
        }
        if reference.daemon_id != daemon_id {
            continue;
        }
        if let Some(existing) = &matched {
            return Err(anyhow!(
                "local_directory: project has multiple local_directory resources for this daemon \
                 ({:?} and {:?}); remove the extra in project settings",
                existing.abs_path,
                reference.local_path.trim(),
            ));
        }
        let abs_path = normalize_local_path(&reference.local_path)?;
        let real_path = resolve_real_path(&abs_path);
        matched = Some(LocalDirectoryAssignment {
            reference,
            abs_path,
            real_path,
        });
    }
    Ok(matched)
}

/// Strips whitespace and resolves to an absolute cleaned form without
/// touching the filesystem (go:162–171).
pub(crate) fn normalize_local_path(path: &str) -> anyhow::Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("local_directory: local_path is empty"));
    }
    if !Path::new(trimmed).is_absolute() {
        return Err(anyhow!(
            "local_directory: local_path must be absolute, got {trimmed:?}"
        ));
    }
    Ok(normalize_lexically(Path::new(trimmed))
        .to_string_lossy()
        .into_owned())
}

/// Symlink-resolved absolute form; falls back to the cleaned absolute path
/// when resolution fails so callers reach the clearer existence error
/// (go:179–188). Never fails, matching the Go fallback contract.
pub(crate) fn resolve_real_path(abs_path: &str) -> String {
    match std::fs::canonicalize(abs_path) {
        Ok(real) => normalize_lexically(&real).to_string_lossy().into_owned(),
        Err(_) => abs_path.to_string(),
    }
}

/// Daemon-side preconditions for running an agent against a user-supplied
/// directory (go:207–257): absolute, not blacklisted literally or after
/// symlink resolution, exists, is a directory, and is read/writable.
pub(crate) fn validate_local_path(abs_path: &str) -> anyhow::Result<()> {
    if abs_path.is_empty() {
        return Err(anyhow!("local_directory: local_path is empty"));
    }
    if !Path::new(abs_path).is_absolute() {
        return Err(anyhow!(
            "local_directory: local_path must be absolute, got {abs_path:?}"
        ));
    }
    if let Some(reason) = is_blacklisted_local_path(abs_path) {
        return Err(anyhow!("local_directory: {reason} ({abs_path:?})"));
    }
    let info = std::fs::metadata(abs_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("local_directory: path does not exist: {abs_path:?}")
        } else {
            anyhow!("local_directory: stat {abs_path:?}: {err}")
        }
    })?;
    if !info.is_dir() {
        return Err(anyhow!(
            "local_directory: path is not a directory: {abs_path:?}"
        ));
    }
    // Re-check the blacklist after resolving symlinks — both a user-created
    // symlink routing writes into a banned target AND the direct-canonical
    // case (/private/tmp on macOS aliasing /tmp) fail closed. EvalSymlinks
    // walks intermediate components, so a parent symlink also fails closed.
    let real_path = std::fs::canonicalize(abs_path)
        .map(|real| normalize_lexically(&real).to_string_lossy().into_owned())
        .map_err(|err| anyhow!("local_directory: resolve symlinks for {abs_path:?}: {err}"))?;
    if let Some(reason) = is_blacklisted_real_path(&real_path) {
        let clean_abs = normalize_lexically(Path::new(abs_path))
            .to_string_lossy()
            .into_owned();
        if real_path != clean_abs {
            return Err(anyhow!(
                "local_directory: {reason} (symlink target of {abs_path:?} is {real_path:?})"
            ));
        }
        return Err(anyhow!(
            "local_directory: {reason} (canonical path {abs_path:?})"
        ));
    }
    check_dir_read_write(abs_path).map_err(|err| anyhow!("local_directory: {:#}", err))?;
    Ok(())
}

/// Literal-equality blacklist after Clean — a legitimate project under
/// `/Users/<user>/code/proj` passes (go:267–283). Returns Some(reason) when
/// blocked.
pub(crate) fn is_blacklisted_local_path(abs_path: &str) -> Option<String> {
    let cleaned = normalize_lexically(Path::new(abs_path));
    if is_drive_root(&cleaned.to_string_lossy()) {
        return Some(format!(
            "path is a drive root {:?}",
            cleaned.display().to_string()
        ));
    }
    for banned in system_root_blacklist() {
        if cleaned == Path::new(banned) {
            return Some(format!("path is a protected system root {banned:?}"));
        }
    }
    if let Some(home) = user_home_dir() {
        if cleaned == normalize_lexically(&home) {
            return Some("path is the user's home directory".to_string());
        }
    }
    None
}

/// Canonical-aware variant comparing the symlink-resolved realPath against
/// the resolved form of each entry, so macOS's /etc→/private/etc family
/// cannot slip past the literal list (go:293–321).
pub(crate) fn is_blacklisted_real_path(real_path: &str) -> Option<String> {
    let real_clean = normalize_lexically(Path::new(real_path));
    if is_drive_root(&real_clean.to_string_lossy()) {
        return Some(format!(
            "path is a drive root {:?}",
            real_clean.display().to_string()
        ));
    }
    for banned in system_root_blacklist() {
        let banned_clean = normalize_lexically(Path::new(banned));
        if real_clean == banned_clean {
            return Some(format!("path is a protected system root {banned:?}"));
        }
        if let Ok(resolved) = std::fs::canonicalize(banned) {
            if normalize_lexically(&resolved) == real_clean {
                return Some(format!("path is a protected system root {banned:?}"));
            }
        }
    }
    if let Some(home) = user_home_dir() {
        let home_clean = normalize_lexically(&home);
        if real_clean == home_clean {
            return Some("path is the user's home directory".to_string());
        }
        if let Ok(resolved) = std::fs::canonicalize(&home_clean) {
            if normalize_lexically(&resolved) == real_clean {
                return Some("path is the user's home directory".to_string());
            }
        }
    }
    None
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

/// Windows volume roots (`C:\`, UNC shares); always false elsewhere because
/// POSIX `/` is covered by [`system_root_blacklist`] (go:334–347).
pub(crate) fn is_drive_root(_abs_path: &str) -> bool {
    #[cfg(windows)]
    {
        let path = Path::new(_abs_path);
        let Some(volume) = path::volume_name(path) else {
            return false;
        };
        let rest = &_abs_path[volume.len()..];
        rest.is_empty() || rest == "\\" || rest == "/"
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod path {
    use std::path::Path;

    /// `filepath.VolumeName`: drive letter or UNC share prefix.
    pub(crate) fn volume_name(path: &Path) -> Option<String> {
        let text = path.to_string_lossy();
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Some(text[..2].to_string());
        }
        // UNC: \\server\share
        if text.starts_with("\\\\") {
            let parts: Vec<&str> = text[2..].splitn(3, ['\\', '/']).collect();
            if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                return Some(format!(r"\\{}\{}", parts[0], parts[1]));
            }
        }
        None
    }
}

/// Per-OS list of paths the daemon never allows as a local_directory root;
/// intentionally conservative since the desktop UI picker should never
/// produce these values (go:357–362).
pub(crate) fn system_root_blacklist() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &[
            r"C:\Users",
            r"C:\ProgramData",
            r"C:\Program Files",
            r"C:\Program Files (x86)",
            r"C:\Windows",
        ]
    }
    #[cfg(not(windows))]
    {
        &[
            "/",
            "/Users",
            "/Users/Shared",
            "/home",
            "/root",
            "/var",
            "/etc",
            "/tmp",
            "/usr",
            "/opt",
        ]
    }
}

/// Verifies the process can read the directory and create/remove a probe
/// file inside it (go:369–381); probe cleanup is best-effort.
pub(crate) fn check_dir_read_write(dir: &str) -> anyhow::Result<()> {
    std::fs::read_dir(dir).with_context(|| format!("read {dir:?}"))?;
    tempfile::Builder::new()
        .prefix(".cordy-rwcheck-")
        .tempfile_in(dir)
        .with_context(|| format!("write {dir:?}"))?;
    Ok(())
}

/// Whether path is the working tree of a git repo (go:389–396); any error
/// means "not a git tree".
pub(crate) async fn is_git_work_tree(ctx: &Ctx, path: &str) -> bool {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["-C", path, "rev-parse", "--is-inside-work-tree"]);
    match crate::gc::processtree::output(ctx, cmd, std::time::Duration::from_secs(2)).await {
        Ok(output) => String::from_utf8_lossy(&output).trim() == "true",
        Err(_) => false,
    }
}

struct PathLockEntry {
    lock: Arc<tokio::sync::Mutex<()>>,
    holder: StdMutex<String>,
}

impl PathLockEntry {
    fn new() -> Self {
        Self {
            lock: Arc::new(tokio::sync::Mutex::new(())),
            holder: StdMutex::new(String::new()),
        }
    }

    fn holder(&self) -> String {
        self.holder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_holder(&self, id: &str) {
        *self
            .holder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = id.to_string();
    }
}

/// Serialises agent tasks that share the same on-disk path, owned for the
/// entire lifetime of a task (claim → context write → agent execution →
/// result report), not just the execution window (go:411–414).
///
/// Entries stay in the map once created: pruning would race with a sibling
/// caller that just looked up the same entry and is about to try-lock, and
/// one entry per distinct served path is tiny in practice (go:528–533).
#[derive(Default)]
pub(crate) struct LocalPathLocker {
    locks: StdMutex<HashMap<String, Arc<PathLockEntry>>>,
}

/// Idempotent release handle returned by [`LocalPathLocker::acquire`]; the
/// Go callback wrapped in sync.Once — explicit [`release`](Self::release)
/// or drop, whichever comes first.
pub(crate) struct PathLockRelease {
    entry: Arc<PathLockEntry>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl std::fmt::Debug for PathLockRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathLockRelease")
            .field("held", &self.guard.is_some())
            .finish()
    }
}

impl PathLockRelease {
    pub fn release(mut self) {
        if let Some(guard) = self.guard.take() {
            self.entry.set_holder("");
            drop(guard);
        }
    }
}

impl Drop for PathLockRelease {
    fn drop(&mut self) {
        if self.guard.take().is_some() {
            self.entry.set_holder("");
        }
    }
}

impl LocalPathLocker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn entry_for(&self, real_path: &str) -> Arc<PathLockEntry> {
        let mut locks = self.locks.lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(
            locks
                .entry(real_path.to_string())
                .or_insert_with(|| Arc::new(PathLockEntry::new())),
        )
    }

    /// The task id currently holding the lock for realPath, or "" when free —
    /// feeds the server-side wait_reason hint (go:431–441).
    pub(crate) fn holder(&self, real_path: &str) -> String {
        let entry = {
            let locks = self.locks.lock().unwrap_or_else(|p| p.into_inner());
            locks.get(real_path).cloned()
        };
        match entry {
            Some(entry) => entry.holder(),
            None => String::new(),
        }
    }

    /// Takes the lock for realPath on behalf of taskID. When contended,
    /// `on_wait` fires once with the current holder before blocking; ctx
    /// cancellation while waiting returns the cancel cause WITHOUT taking
    /// the lock (go:457–515).
    pub(crate) async fn acquire(
        &self,
        ctx: &Ctx,
        real_path: &str,
        task_id: &str,
        on_wait: Option<&(dyn Fn(&str) + Send + Sync)>,
    ) -> anyhow::Result<PathLockRelease> {
        if real_path.is_empty() {
            return Err(anyhow!("local_directory: realpath required for lock"));
        }
        if task_id.is_empty() {
            return Err(anyhow!("local_directory: taskID required for lock"));
        }

        let entry = self.entry_for(real_path);

        // Fast path: uncontended.
        if let Ok(guard) = entry.lock.clone().try_lock_owned() {
            entry.set_holder(task_id);
            return Ok(PathLockRelease {
                entry,
                guard: Some(guard),
            });
        }

        // Slow path: stamp the server-side wait state first.
        if let Some(on_wait) = on_wait {
            on_wait(&entry.holder());
        }

        let acquire = entry.lock.clone().lock_owned();
        tokio::pin!(acquire);
        tokio::select! {
            guard = &mut acquire => {
                entry.set_holder(task_id);
                Ok(PathLockRelease {
                    entry,
                    guard: Some(guard),
                })
            }
            _ = ctx.cancelled() => {
                // We lost the wait — spawn cleanup that releases the moment
                // the acquire lands so nobody queues behind a phantom holder
                // (go:509–512). No holder id is set: this task never owned it.
                let cleanup = entry.lock.clone();
                tokio::spawn(async move {
                    drop(cleanup.lock_owned().await);
                });
                Err(anyhow!("{}", ctx.cause()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Task;
    use serde_json::json;
    use std::time::Duration;

    fn resource(local_path: &str, daemon_id: &str) -> ProjectResourceData {
        ProjectResourceData {
            id: "r1".into(),
            resource_type: LOCAL_DIRECTORY_RESOURCE_TYPE.into(),
            resource_ref: serde_json::to_value(LocalDirectoryRef {
                local_path: local_path.into(),
                daemon_id: daemon_id.into(),
                ..Default::default()
            })
            .unwrap(),
            label: String::new(),
        }
    }

    #[test]
    fn no_resources_returns_none() {
        let got = find_local_directory_assignment(&[], "d-mine").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn other_daemon_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let got = find_local_directory_assignment(
            &[resource(dir.path().to_str().unwrap(), "d-other")],
            "d-mine",
        )
        .unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn non_matching_type_is_skipped() {
        let row = ProjectResourceData {
            id: "r1".into(),
            resource_type: "github_repo".into(),
            resource_ref: json!({"url": "https://x"}),
            label: String::new(),
        };
        let got = find_local_directory_assignment(&[row], "d-mine").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn matching_daemon_returns_assignment() {
        let dir = tempfile::tempdir().unwrap();
        let got = find_local_directory_assignment(
            &[resource(dir.path().to_str().unwrap(), "d-mine")],
            "d-mine",
        )
        .unwrap()
        .expect("assignment");
        assert_eq!(
            got.abs_path,
            normalize_lexically(dir.path()).to_string_lossy()
        );
        assert!(!got.real_path.is_empty());
    }

    #[test]
    fn missing_daemon_id_is_rejected() {
        let row = ProjectResourceData {
            id: "r1".into(),
            resource_type: LOCAL_DIRECTORY_RESOURCE_TYPE.into(),
            resource_ref: json!({"local_path": "/tmp/x"}),
            label: String::new(),
        };
        assert!(find_local_directory_assignment(&[row], "d-mine")
            .err()
            .is_some());
    }

    #[test]
    fn relative_path_is_rejected() {
        assert!(
            find_local_directory_assignment(&[resource("relative/path", "d-mine")], "d-mine")
                .err()
                .is_some()
        );
    }

    #[test]
    fn malformed_ref_fails() {
        let row = ProjectResourceData {
            id: "r1".into(),
            resource_type: LOCAL_DIRECTORY_RESOURCE_TYPE.into(),
            resource_ref: json!("not-an-object"),
            label: String::new(),
        };
        assert!(find_local_directory_assignment(&[row], "d-mine")
            .err()
            .is_some());
    }

    #[test]
    fn two_rows_on_this_daemon_fail_fast() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let err = find_local_directory_assignment(
            &[
                resource(dir_a.path().to_str().unwrap(), "d-mine"),
                resource(dir_b.path().to_str().unwrap(), "d-mine"),
            ],
            "d-mine",
        )
        .unwrap_err();
        assert!(err.to_string().contains("multiple local_directory"));
    }

    #[test]
    fn rows_on_different_daemons_coexist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let got = find_local_directory_assignment(
            &[resource(path, "d-mine"), resource(path, "d-other")],
            "d-mine",
        )
        .unwrap();
        assert!(got.is_some());
    }

    #[test]
    fn squad_leader_tasks_never_bind() {
        let task = Task {
            is_leader_task: true,
            project_resources: vec![resource("/tmp/leader-should-not-bind", "d-mine")],
            ..Default::default()
        };
        let got = local_directory_assignment_for_task(&task, "d-mine").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn validate_local_path_accepts_writable_dir() {
        let dir = tempfile::tempdir().unwrap();
        validate_local_path(dir.path().to_str().unwrap()).unwrap();
    }

    #[test]
    fn validate_local_path_rejects_relative_and_empty() {
        assert!(validate_local_path("relative").is_err());
        assert!(validate_local_path("").is_err());
    }

    #[test]
    fn validate_local_path_rejects_system_roots() {
        for banned in ["/", "/Users", "/home"] {
            if Path::new(banned).exists() {
                assert!(validate_local_path(banned).is_err(), "{banned}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn validate_local_path_rejects_home_and_missing_and_file() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let home = PathBuf::from(home);
        if home.is_dir() {
            assert!(validate_local_path(home.to_str().unwrap()).is_err());
        }

        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(validate_local_path(missing.to_str().unwrap()).is_err());

        let file = dir.path().join("afile");
        std::fs::write(&file, b"hi").unwrap();
        assert!(validate_local_path(file.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validate_local_path_rejects_unwritable_dir() {
        if nix_uid() == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        let err = validate_local_path(dir.path().to_str().unwrap());
        // Restore before asserting so cleanup never fails.
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        assert!(err.is_err());
    }

    fn nix_uid() -> u32 {
        #[cfg(unix)]
        {
            unsafe { libc::getuid() }
        }
        #[cfg(not(unix))]
        {
            1000
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_into_banned_root_is_rejected() {
        if nix_uid() == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // A user-created symlink routing writes into a banned system root
        // fails closed even though the literal path is clean.
        for banned in ["/Users", "/home", "/etc", "/var"] {
            if !Path::new(banned).exists() {
                continue;
            }
            let link = dir.path().join(format!("link{}", banned.replace('/', "-")));
            if std::os::unix::fs::symlink(banned, &link).is_err() {
                continue;
            }
            let err = validate_local_path(link.to_str().unwrap()).unwrap_err();
            assert!(
                err.to_string().contains("protected system root"),
                "{banned}: {err}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn direct_canonical_alias_of_banned_root_is_rejected() {
        // Typing /private/tmp slips past the /tmp literal entry; the
        // canonical-aware check must catch it unconditionally
        // (local_directory.go:227–238).
        let err = validate_local_path("/private/tmp").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("protected system root"), "{text}");
        // And the blacklist helper agrees about the canonical form directly.
        assert!(is_blacklisted_real_path("/private/tmp").is_some());
    }

    #[test]
    fn is_drive_root_is_always_false_on_posix() {
        #[cfg(not(windows))]
        {
            assert!(!is_drive_root("/"));
            assert!(!is_drive_root("C:\\"));
        }
    }

    #[tokio::test]
    async fn locker_serializes_and_reports_waiter_holder() {
        let locker = Arc::new(LocalPathLocker::new());
        let rel1 = locker
            .acquire(&Ctx::new(), "/some/path", "task-1", None)
            .await
            .unwrap();
        assert_eq!(locker.holder("/some/path"), "task-1");

        let seen: Arc<StdMutex<String>> = Arc::new(StdMutex::new(String::new()));
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let tx = StdMutex::new(Some(tx));

        let waiter = {
            let locker = Arc::clone(&locker);
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let on_wait = |holder: &str| {
                    *seen.lock().unwrap_or_else(|p| p.into_inner()) = holder.to_string();
                    if let Some(tx) = tx.lock().unwrap_or_else(|p| p.into_inner()).take() {
                        let _ = tx.send(());
                    }
                };
                let _rel = locker
                    .acquire(&Ctx::new(), "/some/path", "task-2", Some(&on_wait))
                    .await
                    .unwrap();
                locker.holder("/some/path")
            })
        };

        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .expect("onWait never fired")
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap_or_else(|p| p.into_inner()),
            "task-1",
            "onWait holder"
        );

        drop(rel1);
        let holder_after = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter never woke")
            .unwrap();
        assert_eq!(holder_after, "task-2");
    }

    #[tokio::test]
    async fn locker_ctx_cancel_while_waiting() {
        let locker = LocalPathLocker::new();
        let ctx = Ctx::new();
        let rel1 = locker
            .acquire(&ctx, "/some/path", "task-1", None)
            .await
            .unwrap();

        let cancel_ctx = Ctx::new();
        {
            let token = cancel_ctx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                token.cancel_with(crate::repocache::CancelCause::DeadlineExceeded);
            });
        }
        let err = locker
            .acquire(&cancel_ctx, "/some/path", "task-2", None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            crate::repocache::CancelCause::DeadlineExceeded.to_string()
        );
        drop(rel1);
    }

    #[tokio::test]
    async fn distinct_paths_do_not_block() {
        let locker = LocalPathLocker::new();
        let rel1 = locker
            .acquire(&Ctx::new(), "/a", "task-1", None)
            .await
            .unwrap();

        let other = tokio::spawn(async move {
            let l = LocalPathLocker::new();
            let rel = l.acquire(&Ctx::new(), "/b", "task-2", None).await.unwrap();
            rel.release();
        });
        tokio::time::timeout(Duration::from_secs(1), other)
            .await
            .expect("acquire on distinct path blocked")
            .unwrap();
        drop(rel1);
    }

    #[test]
    fn uses_worktree_defaults_to_in_place() {
        let assignment = |mode: &str| LocalDirectoryAssignment {
            reference: LocalDirectoryRef {
                execution_mode: mode.into(),
                ..Default::default()
            },
            abs_path: "/x".into(),
            real_path: "/x".into(),
        };
        assert!(!assignment("").uses_worktree());
        assert!(assignment(LOCAL_DIRECTORY_MODE_WORKTREE).uses_worktree());
        assert!(!assignment(LOCAL_DIRECTORY_MODE_IN_PLACE).uses_worktree());
    }

    #[test]
    fn unknown_execution_mode_rejects_instead_of_in_place() {
        let assignment = LocalDirectoryAssignment {
            reference: LocalDirectoryRef {
                execution_mode: "future-mode".into(),
                ..Default::default()
            },
            abs_path: "/x".into(),
            real_path: "/x".into(),
        };
        let err = assignment.validate_execution_mode().unwrap_err();
        assert!(err.to_string().contains("future-mode"), "{err}");
        assert!(err.to_string().contains("refusing to run in place"));

        for ok in [
            "",
            LOCAL_DIRECTORY_MODE_IN_PLACE,
            LOCAL_DIRECTORY_MODE_WORKTREE,
        ] {
            let assignment = LocalDirectoryAssignment {
                reference: LocalDirectoryRef {
                    execution_mode: ok.into(),
                    ..Default::default()
                },
                abs_path: "/x".into(),
                real_path: "/x".into(),
            };
            assignment.validate_execution_mode().unwrap();
        }
    }
}
