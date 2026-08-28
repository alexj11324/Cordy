//! Machine-scoped daemon identity management.
//!
//! Machine-scoped daemon identity: a stable UUID persisted at
//! `~/.patchbay/daemon.id`, shared by every profile on the machine, with
//! one-time promotion of pre-#1220 per-profile ids and legacy hostname-based
//! id enumeration for server-side row merges.
//!
//! [`profile_dir`] is the daemon-wide contract for resolving identity, GC,
//! and execution-store paths. Keeping those consumers on one resolver avoids
//! platform and task-local environment drift.
//!
//! File bytes are decoded lossily before trimming so a non-UTF-8 daemon id is
//! regenerated instead of surfacing as a read error.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use uuid::Uuid;

/// Stores this machine's stable daemon
/// identifier. Once created, the UUID inside is the daemon's identity forever.
pub(crate) const DAEMON_ID_FILE_NAME: &str = "daemon.id";

/// Optional task-local configuration root.
const TASK_CONFIG_ROOT_ENV: &str = "PATCHBAY_TASK_CONFIG_ROOT";

/// Stable UUID for this daemon instance,
/// persisted on first call. Identity is machine-scoped — every profile shares
/// one UUID at `~/.patchbay/daemon.id`. A corrupt file is regenerated rather than
/// hard-failing startup.
pub(crate) fn ensure_daemon_id(profile: &str) -> anyhow::Result<String> {
    let dir = profile_dir("")?;
    let path = dir.join(DAEMON_ID_FILE_NAME);

    match fs::read(&path) {
        Ok(data) => {
            let id = String::from_utf8_lossy(&data);
            let id = id.trim();
            if !id.is_empty() && Uuid::parse_str(id).is_ok() {
                return Ok(id.to_string());
            }
        }
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(anyhow::Error::new(e).context("read daemon id file"));
        }
        Err(_) => {}
    }

    fs::create_dir_all(&dir).context("create profile directory")?;

    // One-time promotion from pre-change per-profile layout.
    if let Some(promoted) = promote_profile_daemon_id(profile, &path) {
        return Ok(promoted);
    }

    let id = Uuid::now_v7().to_string();
    write_daemon_id_file(&path, &id)?;
    Ok(id)
}

/// Copies a pre-change
/// per-profile daemon.id into the canonical machine-scoped location. Returns
/// None when there is nothing valid to promote (empty profile, missing/corrupt
/// source, any I/O failure) — best-effort, falls through to fresh mint.
fn promote_profile_daemon_id(profile: &str, target_path: &Path) -> Option<String> {
    if profile.is_empty() {
        return None;
    }
    let dir = profile_dir(profile).ok()?;
    let src = dir.join(DAEMON_ID_FILE_NAME);
    let data = fs::read(&src).ok()?;
    let id = String::from_utf8_lossy(&data);
    let id = id.trim();
    if Uuid::parse_str(id).is_err() {
        return None;
    }
    let id = id.to_string();
    write_daemon_id_file(target_path, &id).ok()?;
    Some(id)
}

/// Writes the UUID atomically via temp file + rename, mode 0600. The tempfile
/// guard removes the temp file on any failure path.
fn write_daemon_id_file(path: &Path, id: &str) -> anyhow::Result<()> {
    use std::io::Write;

    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&parent).context("create parent directory")?;
    let tmp = tempfile::Builder::new()
        .prefix(".daemon-")
        .suffix(".id.tmp")
        .tempfile_in(&parent)
        .context("create temp daemon id file")?;
    {
        let mut f = tmp.as_file();
        f.write_all(format!("{id}\n").as_bytes())
            .context("write temp daemon id file")?;
        f.flush().context("write temp daemon id file")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600))
            .context("chmod temp daemon id file")?;
    }
    // persist() renames over the target, matching os.Rename semantics.
    tmp.persist(path)
        .map_err(|e| anyhow::Error::new(e.error).context("rename daemon id file"))?;
    Ok(())
}

/// `LegacyDaemonIDs` (identity.go:152–183): historical daemon_id values this
/// machine may have registered under. `.local` drift is bidirectional, so both
/// bare and `.local`-suffixed forms are always emitted; case drift is handled
/// server-side.
pub(crate) fn legacy_daemon_ids(hostname: &str, profile: &str) -> Vec<String> {
    let host = hostname.trim();
    if host.is_empty() {
        return Vec::new();
    }
    let stripped = host.strip_suffix(".local").unwrap_or(host);
    let dot_local = format!("{stripped}.local");

    let mut candidates = vec![stripped.to_string(), dot_local.clone()];
    if !profile.is_empty() {
        candidates.push(format!("{stripped}-{profile}"));
        candidates.push(format!("{dot_local}-{profile}"));
    }

    let mut seen = std::collections::HashSet::with_capacity(candidates.len());
    candidates
        .into_iter()
        .filter(|c| !c.is_empty() && seen.insert(c.clone()))
        .collect()
}

/// `LegacyDaemonUUIDs` (identity.go:196–226): scan `~/.patchbay/profiles/*/daemon.id`
/// and return every UUID that survives parsing. Per-file errors are swallowed;
/// a missing profiles directory returns an empty list (clean install).
pub(crate) fn legacy_daemon_uuids() -> anyhow::Result<Vec<String>> {
    let root = profile_dir("")?;
    let profiles_dir = root.join("profiles");
    let entries = match fs::read_dir(&profiles_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(anyhow::Error::new(e).context("read profiles dir")),
    };

    let mut ids = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let data = match fs::read(entry.path().join(DAEMON_ID_FILE_NAME)) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let id = String::from_utf8_lossy(&data);
        let id = id.trim();
        if Uuid::parse_str(id).is_err() {
            continue;
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

/// `filterLegacyIDs` (identity.go:231–243): drop entries equal to `current`
/// (nothing to migrate when the row is already keyed on the current id).
pub(crate) fn filter_legacy_ids(ids: Vec<String>, current: &str) -> Vec<String> {
    if current.is_empty() {
        return ids;
    }
    ids.into_iter().filter(|id| id != current).collect()
}

// ---------------------------------------------------------------------------
// Shared profile directory contract.
// ---------------------------------------------------------------------------

/// Base directory for a profile's state files. Empty profile →
/// `<root>/.patchbay`; named profile → `<root>/.patchbay/profiles/<name>`
/// (task-local roots skip the `.patchbay` hop).
pub(crate) fn profile_dir(profile: &str) -> anyhow::Result<PathBuf> {
    let (root, task_local) = patchbay_config_root().context("resolve profile dir")?;
    if task_local {
        validate_task_local_profile(profile).context("resolve profile dir")?;
    }
    if profile.is_empty() {
        return Ok(if task_local {
            root
        } else {
            root.join(".patchbay")
        });
    }
    if task_local {
        Ok(root.join("profiles").join(profile))
    } else {
        Ok(root.join(".patchbay").join("profiles").join(profile))
    }
}

/// Resolves the task-local root when configured, otherwise the platform home.
fn patchbay_config_root() -> anyhow::Result<(PathBuf, bool)> {
    if let Ok(raw_root) = std::env::var(TASK_CONFIG_ROOT_ENV) {
        let raw_root = raw_root.trim();
        if !raw_root.is_empty() {
            let root = PathBuf::from(raw_root);
            if !root.is_absolute() {
                anyhow::bail!("{} must be an absolute path", TASK_CONFIG_ROOT_ENV);
            }
            return Ok((root, true));
        }
    }
    Ok((home_dir()?, false))
}

const fn platform_home_env_key(windows: bool) -> &'static str {
    if windows {
        "USERPROFILE"
    } else {
        "HOME"
    }
}

/// Resolves the native home environment variable for this platform.
fn home_dir() -> anyhow::Result<PathBuf> {
    let key = platform_home_env_key(cfg!(windows));
    std::env::var(key)
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("${} is not defined", key))
}

/// Task-local roots accept only a single safe profile-name segment.
fn validate_task_local_profile(profile: &str) -> anyhow::Result<()> {
    if profile.is_empty() {
        return Ok(());
    }
    if profile == "."
        || profile == ".."
        || Path::new(profile).is_absolute()
        || profile.contains('/')
        || profile.contains('\\')
    {
        anyhow::bail!("invalid task-local Patchbay profile name {:?}", profile);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate HOME — env is process-global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvRestore {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn with_home<F: FnOnce(&Path)>(f: F) {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        const HOME_ENV: &str = "HOME";
        #[cfg(windows)]
        const HOME_ENV: &str = "USERPROFILE";

        let _task_root_restore = EnvRestore {
            key: TASK_CONFIG_ROOT_ENV,
            previous: std::env::var_os(TASK_CONFIG_ROOT_ENV),
        };
        std::env::remove_var(TASK_CONFIG_ROOT_ENV);
        let _home_restore = EnvRestore {
            key: HOME_ENV,
            previous: std::env::var_os(HOME_ENV),
        };
        std::env::set_var(HOME_ENV, home.path());
        f(home.path());
    }

    fn set_task_root(value: impl AsRef<std::ffi::OsStr>) -> EnvRestore {
        let restore = EnvRestore {
            key: TASK_CONFIG_ROOT_ENV,
            previous: std::env::var_os(TASK_CONFIG_ROOT_ENV),
        };
        std::env::set_var(TASK_CONFIG_ROOT_ENV, value);
        restore
    }

    #[test]
    fn profile_dir_ignores_empty_task_root() {
        with_home(|home| {
            let _task_root_restore = set_task_root("");
            assert_eq!(profile_dir("").unwrap(), home.join(".patchbay"));
            assert_eq!(
                profile_dir("staging").unwrap(),
                home.join(".patchbay").join("profiles").join("staging")
            );
        });
    }

    #[test]
    fn profile_dir_prefers_absolute_task_root() {
        with_home(|_| {
            let task_root = tempfile::tempdir().unwrap();
            let _task_root_restore = set_task_root(task_root.path());
            assert_eq!(profile_dir("").unwrap(), task_root.path());
            assert_eq!(
                profile_dir("staging").unwrap(),
                task_root.path().join("profiles").join("staging")
            );
        });
    }

    #[test]
    fn profile_dir_rejects_non_absolute_task_root_and_unsafe_profile() {
        with_home(|_| {
            let _task_root_restore = set_task_root("relative/task-root");
            let err = profile_dir("").unwrap_err();
            let rendered = format!("{err:#}");
            assert!(
                rendered.contains("PATCHBAY_TASK_CONFIG_ROOT must be an absolute path"),
                "{rendered}"
            );

            std::env::set_var(TASK_CONFIG_ROOT_ENV, std::env::temp_dir());
            for profile in [".", "..", "nested/profile", "nested\\profile"] {
                assert!(profile_dir(profile).is_err(), "accepted {profile:?}");
            }
        });
    }

    #[test]
    fn windows_home_uses_userprofile() {
        assert_eq!(platform_home_env_key(true), "USERPROFILE");
        assert_eq!(platform_home_env_key(false), "HOME");
        #[cfg(windows)]
        assert_eq!(platform_home_env_key(cfg!(windows)), "USERPROFILE");
    }

    /// TestEnsureDaemonID_Persists (identity_test.go:14–42).
    #[test]
    fn ensure_daemon_id_persists() {
        with_home(|home| {
            let first = ensure_daemon_id("").unwrap();
            Uuid::parse_str(&first).expect("returned non-UUID");

            let data = fs::read(home.join(".patchbay").join("daemon.id")).unwrap();
            assert_eq!(String::from_utf8_lossy(&data).trim(), first);

            let second = ensure_daemon_id("").unwrap();
            assert_eq!(second, first, "UUID changed on second call");
        });
    }

    /// TestEnsureDaemonID_SharedAcrossProfiles (identity_test.go:44–66).
    #[test]
    fn ensure_daemon_id_shared_across_profiles() {
        with_home(|home| {
            let default_id = ensure_daemon_id("").unwrap();
            let staging_id = ensure_daemon_id("staging").unwrap();
            assert_eq!(
                default_id, staging_id,
                "profiles should share one machine id"
            );

            let profile_file = home
                .join(".patchbay")
                .join("profiles")
                .join("staging")
                .join("daemon.id");
            assert!(
                matches!(fs::metadata(&profile_file), Err(e) if e.kind() == io::ErrorKind::NotFound),
                "profile-scoped daemon.id should not be created"
            );
        });
    }

    /// TestEnsureDaemonID_PromotesPreChangeProfileFile (identity_test.go:68–101).
    #[test]
    fn ensure_daemon_id_promotes_pre_change_profile_file() {
        with_home(|home| {
            let legacy_id = Uuid::now_v7().to_string();
            let profile_dir_path = home.join(".patchbay").join("profiles").join("staging");
            fs::create_dir_all(&profile_dir_path).unwrap();
            fs::write(
                profile_dir_path.join(DAEMON_ID_FILE_NAME),
                format!("{legacy_id}\n"),
            )
            .unwrap();

            let got = ensure_daemon_id("staging").unwrap();
            assert_eq!(got, legacy_id, "expected promoted UUID");

            let data = fs::read(home.join(".patchbay").join("daemon.id")).unwrap();
            assert_eq!(String::from_utf8_lossy(&data).trim(), legacy_id);
        });
    }

    /// TestEnsureDaemonID_RegeneratesCorruptFile (identity_test.go:103–128).
    #[test]
    fn ensure_daemon_id_regenerates_corrupt_file() {
        with_home(|home| {
            let dir = home.join(".patchbay");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(DAEMON_ID_FILE_NAME), b"not-a-uuid").unwrap();

            let id = ensure_daemon_id("").unwrap();
            Uuid::parse_str(&id).expect("expected valid UUID");

            let data = fs::read(dir.join(DAEMON_ID_FILE_NAME)).unwrap();
            assert_eq!(String::from_utf8_lossy(&data).trim(), id);
        });
    }

    /// TestLegacyDaemonUUIDs_ScansProfileDirs (identity_test.go:130–165).
    #[test]
    fn legacy_daemon_uuids_scans_profile_dirs() {
        with_home(|home| {
            let uuid_a = Uuid::now_v7().to_string();
            let uuid_b = Uuid::now_v7().to_string();
            for (name, id) in [("prod", &uuid_a), ("desktop-patchbay", &uuid_b)] {
                let dir = home.join(".patchbay").join("profiles").join(name);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join(DAEMON_ID_FILE_NAME), format!("{id}\n")).unwrap();
            }
            // Corrupt file must be skipped, not fail.
            let corrupt = home.join(".patchbay").join("profiles").join("corrupt");
            fs::create_dir_all(&corrupt).unwrap();
            fs::write(corrupt.join(DAEMON_ID_FILE_NAME), b"not-a-uuid").unwrap();

            let mut got = legacy_daemon_uuids().unwrap();
            got.sort();
            let mut want = vec![uuid_a, uuid_b];
            want.sort();
            assert_eq!(got, want);
        });
    }

    /// TestLegacyDaemonUUIDs_MissingProfilesDirIsNil (identity_test.go:167–178).
    #[test]
    fn legacy_daemon_uuids_missing_profiles_dir_is_nil() {
        with_home(|_| {
            assert!(legacy_daemon_uuids().unwrap().is_empty());
        });
    }

    /// TestLegacyDaemonIDs table (identity_test.go:180–242).
    #[test]
    fn legacy_daemon_ids_table() {
        let cases: &[(&str, &str, &str, Vec<&str>)] = &[
            (
                "plain hostname, no profile",
                "MacBook-Pro",
                "",
                vec!["MacBook-Pro", "MacBook-Pro.local"],
            ),
            (
                "dot-local hostname, no profile",
                "MacBook-Pro.local",
                "",
                vec!["MacBook-Pro", "MacBook-Pro.local"],
            ),
            (
                "plain hostname with profile",
                "MacBook-Pro",
                "staging",
                vec![
                    "MacBook-Pro",
                    "MacBook-Pro.local",
                    "MacBook-Pro-staging",
                    "MacBook-Pro.local-staging",
                ],
            ),
            (
                "dot-local hostname with profile",
                "MacBook-Pro.local",
                "staging",
                vec![
                    "MacBook-Pro",
                    "MacBook-Pro.local",
                    "MacBook-Pro-staging",
                    "MacBook-Pro.local-staging",
                ],
            ),
            ("empty hostname", "", "", vec![]),
            (
                "mixed case hostname preserved as-is",
                "Jiayuans-MacBook-Pro.local",
                "",
                vec!["Jiayuans-MacBook-Pro", "Jiayuans-MacBook-Pro.local"],
            ),
        ];
        for (name, hostname, profile, want) in cases {
            let got = legacy_daemon_ids(hostname, profile);
            let want: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            assert_eq!(&got, &want, "case {name:?}");
        }
    }

    /// filterLegacyIDs behavior (identity.go:228–243).
    #[test]
    fn filter_legacy_ids_drops_current() {
        let ids = vec!["a".into(), "b".into()];
        assert_eq!(filter_legacy_ids(ids.clone(), ""), ids);
        assert_eq!(filter_legacy_ids(ids, "a"), vec!["b".to_string()]);
    }
}
