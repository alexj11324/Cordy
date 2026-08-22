//! Port of `server/internal/daemon/config.go` (in progress — lane A2).
//!
//! Chunk 1 seeded first so `canonical_path.rs` compiles: the executable-path
//! helpers (config.go:815–855). The full `Config` struct + loader
//! (config.go:1–814, 857–1143) lands as later chunks in this same file.

// S9-integration: consumed by daemon bootstrap wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// `canonicalExecutablePath` (config.go:835–847): absolutize, then resolve
/// symlinks; on failure keep the previous-best path and log at debug.
pub(crate) fn canonical_executable_path(path: &str) -> String {
    let abs = match absolute(path) {
        Ok(abs) => abs,
        Err(err) => {
            tracing::debug!(
                path = %path,
                error = %err,
                "make agent executable path absolute failed; keeping configured path"
            );
            return path.to_string();
        }
    };
    let abs_str = abs.to_string_lossy().into_owned();
    match crate::canonical_path::canonical_path(&abs_str) {
        Ok(real) => real.to_string_lossy().into_owned(),
        Err(err) => {
            tracing::debug!(
                path = %abs_str,
                error = %err,
                "canonicalize agent executable path failed; keeping absolute path"
            );
            abs_str
        }
    }
}

/// `isExecutableFile` (config.go:849–855): exists, not a directory, and has
/// any execute bit set. Windows has no POSIX mode bits — Go's os.Stat on
/// Windows synthesizes them from the file extension; here we only check
/// existence + non-directory there.
pub(crate) fn is_executable_file(path: &str) -> bool {
    let Ok(info) = std::fs::metadata(path) else {
        return false;
    };
    if info.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `samePathDir` (config.go:815–833): lexical-absolutize both sides, then
/// best-effort symlink resolution before comparing.
pub(crate) fn same_path_dir(a: &str, b: &str) -> bool {
    use std::path::Component;

    fn clean_abs(p: &str) -> Option<PathBuf> {
        let raw = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            std::env::current_dir().ok()?.join(p)
        };
        let mut out = PathBuf::new();
        for comp in raw.components() {
            match comp {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !matches!(out.components().next_back(), Some(Component::Normal(_))) {
                        out.push("..");
                    } else {
                        out.pop();
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        Some(out)
    }

    let mut abs_a = match clean_abs(a) {
        Some(p) => p,
        None => return false,
    };
    let mut abs_b = match clean_abs(b) {
        Some(p) => p,
        None => return false,
    };
    if let Ok(real) = std::fs::canonicalize(&abs_a) {
        abs_a = real;
    }
    if let Ok(real) = std::fs::canonicalize(&abs_b) {
        abs_b = real;
    }
    abs_a == abs_b
}

/// `filepath.Abs` equivalent shared by config-side callers (kept local to this
/// module; canonical_path.rs has its own copy for its seam).
fn absolute(path: &str) -> std::io::Result<PathBuf> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(p))
}
