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
/// fallback fail. Both errors remain structured: the fallback is the standard
/// error source chain, while the original OS error is available through
/// [`ResolveError::executable_error`].
#[derive(Debug)]
pub struct ResolveError {
    executable: io::Error,
    fallback: FallbackError,
}

/// The structured error returned by the `argv[0]` lookup.
#[derive(Debug)]
pub struct FallbackError {
    argv0: OsString,
    source: io::Error,
}

impl fmt::Display for FallbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "resolve argv[0] {:?}: {}",
            self.argv0, self.source
        )
    }
}

impl std::error::Error for FallbackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl FallbackError {
    /// The original argv[0] supplied by the launcher.
    pub fn argv0(&self) -> &OsStr {
        &self.argv0
    }

    /// The underlying lookup/stat/access error.
    pub fn source_error(&self) -> &io::Error {
        &self.source
    }
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
        Some(&self.fallback)
    }
}

impl ResolveError {
    /// Return the original `os.Executable` error.
    pub fn executable_error(&self) -> &io::Error {
        &self.executable
    }

    /// Return the structured `argv[0]` fallback error.
    pub fn fallback_error(&self) -> &FallbackError {
        &self.fallback
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
                fallback: FallbackError {
                    argv0: OsString::new(),
                    source: io::Error::new(io::ErrorKind::InvalidInput, "argv[0] is empty"),
                },
            }),
            Some(argv0) => match resolve_argv0(&argv0, path.as_deref(), current_dir) {
                Ok(path) => Ok(path),
                Err(error) => Err(ResolveError {
                    executable,
                    fallback: FallbackError {
                        argv0,
                        source: error,
                    },
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
        #[cfg(windows)]
        {
            find_candidate(Path::new(""), argv0)?
        }
        #[cfg(not(windows))]
        {
            PathBuf::from(argv0)
        }
    } else {
        find_on_path(argv0, path)?
    };
    let absolute = absolutize(&candidate, current_dir)?;
    validate_fallback_executable(&absolute)?;
    Ok(absolute)
}

fn find_on_path(name: &OsStr, path: Option<&OsStr>) -> io::Result<PathBuf> {
    let path = path.unwrap_or_else(|| OsStr::new(""));

    #[cfg(windows)]
    let mut dot_candidate = None;

    #[cfg(windows)]
    if std::env::var_os("NoDefaultCurrentDirectoryInExePath").is_none() {
        if let Ok(candidate) = find_candidate(Path::new("."), name) {
            dot_candidate = Some(candidate);
        }
    }

    for directory in std::env::split_paths(path) {
        #[cfg(not(windows))]
        let directory = if directory.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            directory
        };

        #[cfg(windows)]
        if directory.as_os_str().is_empty() {
            continue;
        }

        match find_candidate(&directory, name) {
            Ok(candidate) => {
                if candidate.is_relative() {
                    #[cfg(windows)]
                    {
                        if dot_candidate.is_none() {
                            dot_candidate = Some(candidate);
                        }
                        continue;
                    }
                    #[cfg(not(windows))]
                    return Err(err_dot(candidate));
                }
                return Ok(candidate);
            }
            Err(_) => {}
        }
    }

    #[cfg(windows)]
    if let Some(candidate) = dot_candidate {
        return Err(err_dot(candidate));
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "executable file not found in PATH",
    ))
}

fn find_candidate(directory: &Path, name: &OsStr) -> io::Result<PathBuf> {
    for candidate in path_candidates(directory, name) {
        if validate_lookup_executable(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "executable candidate not found",
    ))
}

fn err_dot(path: PathBuf) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "cannot run executable found relative to current directory: {}",
            path.display()
        ),
    )
}

#[cfg(not(windows))]
fn path_candidates(directory: &Path, name: &OsStr) -> Vec<PathBuf> {
    vec![directory.join(name)]
}

#[cfg(windows)]
fn path_candidates(directory: &Path, name: &OsStr) -> Vec<PathBuf> {
    path_candidates_with(directory, name, &pathext())
}

#[cfg(windows)]
fn path_candidates_with(directory: &Path, name: &OsStr, extensions: &[OsString]) -> Vec<PathBuf> {
    let path = directory.join(name);
    let literal = Path::new(name).extension().is_some().then_some(path);
    literal
        .into_iter()
        .chain(extensions.iter().map(|extension| {
            let mut candidate = directory.join(name).as_os_str().to_os_string();
            candidate.push(extension);
            PathBuf::from(candidate)
        }))
        .collect()
}

#[cfg(windows)]
fn pathext() -> Vec<OsString> {
    parse_pathext(std::env::var_os("PATHEXT").as_deref())
}

#[cfg(windows)]
fn parse_pathext(value: Option<&OsStr>) -> Vec<OsString> {
    match value {
        Some(value) if !value.is_empty() => value
            .to_string_lossy()
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| {
                let extension = extension.to_ascii_lowercase();
                if extension.starts_with('.') {
                    OsString::from(extension)
                } else {
                    OsString::from(format!(".{extension}"))
                }
            })
            .collect(),
        _ => [".com", ".exe", ".bat", ".cmd"]
            .into_iter()
            .map(OsString::from)
            .collect(),
    }
}

fn validate_fallback_executable(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{} is not a regular file", path.display()),
        ));
    }

    validate_executable(path, &metadata)
}

fn validate_lookup_executable(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            format!("{} is a directory", path.display()),
        ));
    }

    validate_executable(path, &metadata)
}

fn validate_executable(_path: &Path, _metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let c_path = CString::new(_path.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "executable path contains NUL")
        })?;
        if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } != 0 {
            let access_error = io::Error::last_os_error();
            let access_errno = access_error.raw_os_error();
            if !matches!(access_errno, Some(code) if code == libc::ENOSYS || code == libc::EPERM) {
                return Err(access_error);
            }
            if _metadata.permissions().mode() & 0o111 == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is not executable", _path.display()),
                ));
            }
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
            Ok(directory.clone()),
        );
        assert_eq!(result.unwrap(), expected);
        fs::remove_file(expected).unwrap();
        fs::remove_dir(directory).unwrap();
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
        fs::remove_file(expected).unwrap();
        fs::remove_dir(directory).unwrap();
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
        fs::remove_file(expected).unwrap();
        fs::remove_dir(directory).unwrap();
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
        assert_eq!(error.executable_error().kind(), io::ErrorKind::NotFound);
        assert_eq!(
            error.fallback_error().argv0(),
            OsStr::new("cordy-does-not-exist")
        );
        assert!(error.fallback_error().source().is_some());
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

    #[test]
    fn rejects_missing_argv0() {
        let error = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing executable",
            )),
            None,
            None,
            Ok(PathBuf::from("/tmp")),
        )
        .unwrap_err();

        assert_eq!(error.fallback_error().argv0(), OsStr::new(""));
        assert_eq!(
            error.fallback_error().source_error().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(error.source().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_relative_path_entries_with_go_errdot_semantics() {
        let root = std::env::current_dir().unwrap();
        let relative = format!("cordy-self-exec-relative-{}", unique_suffix());
        let directory = root.join(&relative);
        fs::create_dir_all(&directory).unwrap();
        let expected = directory.join("cordy");
        executable(&expected);

        let result = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing executable",
            )),
            Some(OsString::from("cordy")),
            Some(relative.into()),
            Ok(root),
        );
        let error = result.unwrap_err();
        assert_eq!(
            error.fallback_error().source_error().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(error
            .fallback_error()
            .source_error()
            .to_string()
            .contains("relative to current directory"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_empty_path_entry_with_go_errdot_semantics() {
        let root = std::env::current_dir().unwrap();
        let name = format!("cordy-self-exec-empty-path-{}", unique_suffix());
        let expected = root.join(&name);
        executable(&expected);

        let result = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing executable",
            )),
            Some(OsString::from(&name)),
            Some(OsString::new()),
            Ok(root),
        );
        let error = result.unwrap_err();
        assert!(error
            .fallback_error()
            .source_error()
            .to_string()
            .contains("relative to current directory"));
        fs::remove_file(expected).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_executable_regular_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile_dir();
        let expected = directory.join("cordy");
        fs::write(&expected, b"not executable").unwrap();
        let mut permissions = fs::metadata(&expected).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&expected, permissions).unwrap();

        let error = resolve_with(
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing executable",
            )),
            Some(expected.clone().into_os_string()),
            None,
            Ok(directory.clone()),
        )
        .unwrap_err();
        assert_eq!(
            error.fallback_error().source_error().kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::remove_file(expected).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_candidates_preserve_pathext_order_and_existing_extensions() {
        let candidates = path_candidates_with(
            Path::new(r"C:\bin"),
            OsStr::new("tool.exe"),
            &[OsString::from(".com"), OsString::from(".exe")],
        );
        let names = candidates
            .iter()
            .map(|path| path.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                r"C:\bin\tool.exe",
                r"C:\bin\tool.exe.com",
                r"C:\bin\tool.exe.exe"
            ]
        );
        assert!(has_path_separator(OsStr::new(r"C:\bin\tool")));
        assert!(has_path_separator(OsStr::new(r"bin\tool")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_bare_names_require_a_pathext_suffix() {
        let candidates = path_candidates_with(
            Path::new(r"C:\bin"),
            OsStr::new("tool"),
            &[OsString::from(".com"), OsString::from(".exe")],
        );
        let names = candidates
            .iter()
            .map(|path| path.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![r"C:\bin\tool.com", r"C:\bin\tool.exe"]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_pathext_parser_normalizes_case_and_dot_prefix() {
        assert_eq!(
            parse_pathext(Some(OsStr::new(".EXE;bat;.;"))),
            vec![
                OsString::from(".exe"),
                OsString::from(".bat"),
                OsString::from(".")
            ]
        );
        assert_eq!(
            parse_pathext(Some(OsStr::new(""))),
            vec![
                OsString::from(".com"),
                OsString::from(".exe"),
                OsString::from(".bat"),
                OsString::from(".cmd")
            ]
        );
    }

    fn tempfile_dir() -> PathBuf {
        let suffix = unique_suffix();
        let directory = std::env::current_dir()
            .unwrap()
            .join(format!(".cordy-self-exec-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
