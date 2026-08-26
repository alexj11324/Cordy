use anyhow::{bail, Result};
use std::fs;
use std::path::{Component, Path, PathBuf};
pub(super) fn ensure_file_within_workdir(
    file_path: &Path,
    current_dir: &Path,
    allow_external_file: bool,
    field: &str,
) -> Result<()> {
    if allow_external_file {
        return Ok(());
    }
    let base = fs::canonicalize(current_dir).unwrap_or_else(|_| lexical_normalize(current_dir));
    let absolute = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        current_dir.join(file_path)
    };
    let candidate = fs::canonicalize(&absolute).unwrap_or_else(|_| {
        let parent = absolute.parent().unwrap_or(current_dir);
        let parent = fs::canonicalize(parent).unwrap_or_else(|_| lexical_normalize(parent));
        absolute
            .file_name()
            .map_or_else(|| lexical_normalize(&absolute), |name| parent.join(name))
    });
    if !candidate.starts_with(&base) {
        let flag = if field == "file" {
            field.to_owned()
        } else {
            format!("{field}-file")
        };
        bail!(
            "--{flag} path {:?} resolves outside the current working directory; write agent temp files inside the task workdir (e.g. ./{field}.md) rather than machine-shared paths like /tmp, where another run's stale file can be read by mistake. Pass --allow-external-file to override.",
            file_path,
        );
    }
    Ok(())
}

pub(super) fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
