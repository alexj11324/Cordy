//! Port of `server/internal/daemon/canonical_path.go` (lines 1–61) and
//! `server/internal/daemon/canonical_path_windows.go` (lines 1–127).
//!
//! Canonicalizes agent executable paths. Ordinary symlinks resolve to their
//! final target, but entrypoints backed by a name-dispatching shim (Volta's
//! `volta-shim`, Vite Plus's `vp`) keep the invoked basename — spawning the
//! shim's target directly turns `claude --version` into `volta-shim
//! --version` (#6183, #6702).
//!
//! Deviations from Go:
//! - `filepath.EvalSymlinks` → [`std::fs::canonicalize`], which additionally
//!   absolutizes; every Go caller passes an absolute path, so behavior is
//!   unchanged in practice.
//! - The Windows build uses `std::fs::canonicalize` (which wraps
//!   CreateFileW + GetFinalPathNameByHandleW internally) plus the same
//!   extended-length-prefix trimming, instead of calling the Win32 APIs
//!   directly (no windows-sys dependency available to this crate).
//! - Go's test-injection var `executablePathForLaunch` becomes a plain
//!   function returning `Option`; integration swaps the implementation at
//!   the call site.

// S9-integration: consumed by daemon.go executable discovery wiring that
// lands with integration; silence dead-code until then.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Context;

/// `canonicalPath` (canonical_path.go:10–12 / canonical_path_windows.go:74–104):
/// resolve the path to its final filesystem location.
#[cfg(not(windows))]
pub(crate) fn canonical_path(path: &str) -> anyhow::Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("evalsymlinks {}", path))
}

/// Windows variant (canonical_path_windows.go:74–104): resolves via
/// GetFinalPathNameByHandleW (inside `fs::canonicalize`) and trims the
/// extended-length prefix when it is not load-bearing (#6883).
#[cfg(windows)]
pub(crate) fn canonical_path(path: &str) -> anyhow::Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)
        .with_context(|| format!("GetFinalPathNameByHandle {}", path))?;
    Ok(PathBuf::from(trim_extended_length_prefix(
        &resolved.to_string_lossy(),
    )))
}

/// `discoveredExecutablePath` (canonical_path.go:25–39): canonicalize ordinary
/// executable symlinks but preserve entrypoints backed by a known
/// name-dispatching shim; the parent directory is still canonicalized so
/// paths stay stable across symlinked/ephemeral version-manager prefixes.
#[cfg(not(windows))]
pub(crate) fn discovered_executable_path(path: &str) -> String {
    let real = crate::config::canonical_executable_path(path);
    if !is_name_dispatching_agent_shim(&real) {
        return real;
    }
    let abs = match absolute(path) {
        Ok(abs) => abs,
        Err(_) => return real,
    };
    let real_dir = match abs.parent().map(|d| std::fs::canonicalize(d)) {
        Some(Ok(dir)) => dir,
        _ => return abs.to_string_lossy().into_owned(),
    };
    let base = abs
        .file_name()
        .map(|b| b.to_os_string())
        .unwrap_or_default();
    real_dir.join(base).to_string_lossy().into_owned()
}

/// Windows variant (canonical_path_windows.go:106–112): plain absolutization.
#[cfg(windows)]
pub(crate) fn discovered_executable_path(path: &str) -> String {
    match absolute(path) {
        Ok(abs) => abs.to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

/// `isNameDispatchingAgentShim` (canonical_path.go:41–51): exact-match list of
/// dispatchers confirmed to require the invoked entrypoint name. Extensions
/// are intentionally NOT stripped — these managers use wrappers or
/// trampolines rather than name-dispatching symlinks.
fn is_name_dispatching_agent_shim(path: &str) -> bool {
    let base = Path::new(path)
        .file_name()
        .map(|b| b.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    base == "volta-shim" || base == "vp"
}

/// `executablePathForLaunch` (canonical_path.go:53–57 /
/// canonical_path_windows.go:118–127): platform hook used when launching a
/// discovered agent binary. `Ok(None)` mirrors Go's `(\"\", false, nil)`
/// "not handled" result.
///
/// Non-Windows default never handles; Windows absolutizes + canonicalizes and
/// always reports handled.
#[cfg(not(windows))]
pub(crate) fn executable_path_for_launch(_path: &str) -> anyhow::Result<Option<String>> {
    Ok(None)
}

#[cfg(windows)]
pub(crate) fn executable_path_for_launch(path: &str) -> anyhow::Result<Option<String>> {
    let abs = absolute(path).context("make launch path absolute")?;
    let resolved = canonical_path(&abs.to_string_lossy())?;
    Ok(Some(resolved.to_string_lossy().into_owned()))
}

/// `canonicalConfiguredExecutablePath` (canonical_path.go:59–61 /
/// canonical_path_windows.go:114–116): how a user-configured executable path
/// is normalized before comparisons.
#[cfg(not(windows))]
pub(crate) fn canonical_configured_executable_path(path: &str) -> String {
    path.to_string()
}

#[cfg(windows)]
pub(crate) fn canonical_configured_executable_path(path: &str) -> String {
    discovered_executable_path(path)
}

/// `filepath.Abs` equivalent: absolutize without resolving symlinks.
fn absolute(path: &str) -> std::io::Result<PathBuf> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(normalize_dots(p));
    }
    let cwd = std::env::current_dir()?;
    Ok(normalize_dots(&cwd.join(p)))
}

/// Lexical `filepath.Clean` for the components `filepath.Abs` collapses:
/// drops `.` segments and resolves `..` lexically against what precedes them.
fn normalize_dots(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop when the tail is an ordinary component; keep
                // leading ParentDirs on relative paths verbatim.
                if out.components().next_back() == Some(Component::RootDir)
                    || !matches!(
                        out.components().next_back(),
                        Some(Component::Normal(_))
                    )
                {
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Windows-only helpers (canonical_path_windows.go:14–72).
// ---------------------------------------------------------------------------

/// `extendedLengthPrefix` / `extendedLengthUNCPrefix`
/// (canonical_path_windows.go:17–20).
#[cfg(windows)]
const EXTENDED_LENGTH_PREFIX: &str = r"\\?\";
#[cfg(windows)]
const EXTENDED_LENGTH_UNC_PREFIX: &str = r"\\?\UNC\";

/// `syscall.MAX_PATH` on Windows.
#[cfg(windows)]
const MAX_PATH: usize = 260;

/// `trimExtendedLengthPrefix` (canonical_path_windows.go:33–52): return the
/// plain Win32 form when short enough not to need the extended-length prefix,
/// unchanged otherwise. cmd.exe rejects the prefix outright and `.cmd` shims
/// cannot execute it, but past MAX_PATH it is the only way to name the file.
#[cfg(windows)]
fn trim_extended_length_prefix(path: &str) -> String {
    let trimmed: String;
    if let Some(rest) = path.strip_prefix(EXTENDED_LENGTH_UNC_PREFIX) {
        trimmed = format!(r"\\{}", rest);
    } else if let Some(rest) = path.strip_prefix(EXTENDED_LENGTH_PREFIX) {
        // Only a drive-qualified path (`C:\...`) is a valid Win32 path without
        // the prefix. Anything else (a volume GUID, say) must keep it.
        let bytes = rest.as_bytes();
        if bytes.len() < 3 || bytes[1] != b':' || !is_path_separator(bytes[2]) {
            return path.to_string();
        }
        trimmed = rest.to_string();
    } else {
        return path.to_string();
    }
    if !fits_without_extended_length_prefix(&trimmed) {
        return path.to_string();
    }
    trimmed
}

/// `fitsWithoutExtendedLengthPrefix` (canonical_path_windows.go:63–72): MAX_PATH
/// counts UTF-16 code units including the terminating NUL, so this must not
/// measure UTF-8 bytes — CJK paths would be over-counted and keep a prefix
/// they do not need.
#[cfg(windows)]
fn fits_without_extended_length_prefix(path: &str) -> bool {
    // Embedded NULs are impossible in real paths; treat as "keep the prefix".
    if path.contains('\0') {
        return false;
    }
    path.encode_utf16().count() + 1 <= MAX_PATH
}

/// `os.IsPathSeparator`.
#[cfg(windows)]
fn is_path_separator(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TestIsNameDispatchingAgentShim_RequiresExactDispatcherName
    /// (canonical_path_test.go:10–32).
    #[test]
    fn name_dispatching_shim_requires_exact_dispatcher_name() {
        let cases = [
            ("manager/volta-shim", true),
            ("manager/vp", true),
            ("manager/VP", true),
            ("manager/vpn", false),
            ("manager/vproxy", false),
            ("manager/volta-shim.exe", false),
            ("manager/claude-2.1.216", false),
        ];
        for (path, want) in cases {
            assert_eq!(
                is_name_dispatching_agent_shim(path),
                want,
                "isNameDispatchingAgentShim({path:?})"
            );
        }
    }

    #[test]
    fn normalize_dots_matches_filepath_abs_shapes() {
        assert_eq!(normalize_dots(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize_dots(Path::new("/a/./b/")), PathBuf::from("/a/b"));
        assert_eq!(
            normalize_dots(Path::new("rel/../x")),
            PathBuf::from("x")
        );
    }

    #[cfg(windows)]
    #[test]
    fn trim_prefix_keeps_long_paths_and_volume_guids() {
        // Drive-qualified short path loses its prefix.
        assert_eq!(
            trim_extended_length_prefix(r"\\?\C:\dir\file"),
            r"C:\dir\file"
        );
        // UNC form becomes a normal UNC path.
        assert_eq!(
            trim_extended_length_prefix(r"\\?\UNC\server\share\file"),
            r"\\server\share\file"
        );
        // Volume GUID keeps the prefix (not drive-qualified).
        let guid = r"\\?\Volume{00000000-0000-0000-0000-000000000000}\file";
        assert_eq!(trim_extended_length_prefix(guid), guid);
        // Past MAX_PATH UTF-16 units the prefix stays.
        let long = format!(r"\\?\C:\{}", "d".repeat(MAX_PATH));
        assert_eq!(trim_extended_length_prefix(&long), long);
    }
}
