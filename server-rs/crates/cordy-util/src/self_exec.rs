//! Resolve the executable backing the current process.
//!
//! Port of `server/internal/selfexec`. Launchers can omit the OS-reported
//! executable metadata, so the resolver falls back to `argv[0]` with the same
//! PATH and regular-file checks used by the Go implementation. Keeping this
//! in the shared utility crate gives the daemon and migration binary one
//! process-location contract.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Error returned when both the OS executable lookup and the `argv[0]`
/// fallback fail. The OS error is retained as the source so callers can
/// inspect the original failure while the display text preserves the Go
/// resolver's joined-error shape.
#[derive(Debug)]
pub struct ResolveError {
    executable: io::Error,
    fallback: String,
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "os.Executable: {}\n{}",
            self.executable, self.fallback
        )
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.executable)
    }
}

/// Resolve the executable that should be used for a self-restart or helper
/// subprocess.
pub fn resolve() -> Result<PathBuf, ResolveError> {
    resolve_with(
        std::env::current_exe(),
        std::env::args_os().next(),
        std::env::var_os("PATH"),
        std::env::current_dir(),
    )
}

fn resolve_with(
    executable: io::Result<PathBuf>,
    argv0: Option<OsString>,
    path: Option<OsString>,
    current_dir: io::Result<PathBuf>,
) -> Result<PathBuf, ResolveError> {
    match executable {
        Ok(path) => Ok(path),
        Err(executable) => match argv0.filter(|value| !value.is_empty()) {
            None => Err(ResolveError {
                executable,
                fallback: "argv[0] is empty".to_string(),
            }),
            Some(argv0) => match resolve_argv0(&argv0, path.as_deref(), current_dir) {
                Ok(path) => Ok(path),
                Err(error) => Err(ResolveError {
                    executable,
                    fallback: format!("resolve argv[0] {argv0:?}: {error}"),
                }),
            },
        },
    }
}

fn resolve_argv0(
    argv0: &OsStr,
    path: Option<&OsStr>,
    current_dir: io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let candidate = if has_path_separator(argv0) {
        PathBuf::from(argv0)
    } else {
        find_on_path(argv0, path)?
    };
    let absolute = absolutize(&candidate, current_dir)?;
    validate_fallback_executable(&absolute)?;
    Ok(absolute)
}

fn find_on_path(name: &OsStr, path: Option<&OsStr>) -> io::Result<PathBuf> {
    let path = path.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "PATH is not set"))?;
    let mut permission_denied = false;

    for directory in std::env::split_paths(path) {
        for candidate in path_candidates(&directory, name) {
            match validate_fallback_executable(&candidate) {
                Ok(()) => return Ok(candidate),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    permission_denied = true;
                }
                Err(_) => {}
            }
        }
    }

    if permission_denied {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "executable file is not executable",
        ))
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "executable file not found in PATH",
        ))
    }
}

#[cfg(not(windows))]
fn path_candidates(directory: &Path, name: &OsStr) -> Vec<PathBuf> {
    vec![directory.join(name)]
}

#[cfg(windows)]
fn path_candidates(directory: &Path, name: &OsStr) -> Vec<PathBuf> {
    let name = Path::new(name);
    if name.extension().is_some() {
        return vec![directory.join(name)];
    }

    let extensions = std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);

    std::iter::once(directory.join(name))
        .chain(extensions.into_iter().map(|extension| {
            let mut candidate = directory.join(name);
            candidate.set_extension(extension.trim_start_matches('.'));
            candidate
        }))
        .collect()
}

fn validate_fallback_executable(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not executable", path.display()),
            ));
        }
    }

    Ok(())
}

fn absolutize(path: &Path, current_dir: io::Result<PathBuf>) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir?.join(path)
    };
    Ok(normalize_absolute(&path))
}

fn normalize_absolute(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.parent().is_some() {
                    normalized.pop();
                }
            }
            Component::Normal(value) => normalized.push(value),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(not(windows))]
fn has_path_separator(value: &OsStr) -> bool {
    value.to_string_lossy().contains('/')
}

#[cfg(windows)]
fn has_path_separator(value: &OsStr) -> bool {
    let value = value.to_string_lossy();
    value.contains('/') || value.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    fn executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    #[test]
    fn uses_os_executable_without_touching_the_file() {
        let expected = PathBuf::from("/does/not/need/to/exist");
        let result = resolve_with(Ok(expected.clone()), None, None, Ok(PathBuf::from("/tmp")));
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn falls_back_to_absolute_argv0() {
        let directory = tempfile_dir();
        let expected = directory.join("cordy");
        executable(&expected);

        let result = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find executable path",
            )),
            Some(expected.clone().into_os_string()),
            None,
            Ok(directory),
        );
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn falls_back_to_relative_argv0_and_normalizes_it() {
        let directory = tempfile_dir();
        let expected = directory.join("cordy");
        executable(&expected);

        let result = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find executable path",
            )),
            Some(OsString::from("./cordy")),
            None,
            Ok(directory.clone()),
        );
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn searches_path_for_bare_argv0() {
        let directory = tempfile_dir();
        let expected = directory.join("cordy");
        executable(&expected);

        let result = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find executable path",
            )),
            Some(OsString::from("cordy")),
            Some(directory.clone().into_os_string()),
            Ok(PathBuf::from("/tmp")),
        );
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn preserves_os_error_and_describes_invalid_fallbacks() {
        let error = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find executable path",
            )),
            Some(OsString::from("cordy-does-not-exist")),
            Some(OsString::from("/empty")),
            Ok(PathBuf::from("/tmp")),
        )
        .unwrap_err();

        assert!(error.source().is_some());
        assert!(error
            .to_string()
            .contains("os.Executable: cannot find executable path"));
        assert!(error.to_string().contains("cordy-does-not-exist"));
    }

    #[test]
    fn rejects_empty_argv0() {
        let error = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find executable path",
            )),
            Some(OsString::new()),
            None,
            Ok(PathBuf::from("/tmp")),
        )
        .unwrap_err();
        assert!(error.to_string().contains("argv[0] is empty"));
    }

    fn tempfile_dir() -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("cordy-self-exec-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }
}
