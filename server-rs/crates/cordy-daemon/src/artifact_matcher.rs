#![allow(dead_code)] // S9-integration: consumed by daemon.go core wiring (S8)
//! Port of `server/internal/daemon/artifact_matcher.go` — combines
//! operator-configured basename matches with exact daemon-managed paths.
//! Exact paths take precedence so a broad basename such as `.sandbox-bin`
//! cannot double-count a managed directory.
//!
//! Symbol map:
//! - `artifactMatcher` → [`ArtifactMatcher`]
//! - `newArtifactMatcher` → [`ArtifactMatcher::new`]
//! - `matchDirectory` → [`ArtifactMatcher::match_directory`]
//! - `managedSubpaths` → [`ArtifactMatcher::managed_subpaths`]
//! - `safeRelativePath` → [`safe_relative_path`]

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub(crate) const MANAGED_ARTIFACT_PATTERN_PREFIX: &str = "managed:";

#[derive(Debug, Clone, Default)]
pub(crate) struct ArtifactMatcher {
    basenames: HashSet<String>,
    /// rel path (OS separators) → `managed:<slash display path>`
    exact_paths: HashMap<PathBuf, String>,
    exact_leaf_names: HashSet<String>,
}

impl ArtifactMatcher {
    pub(crate) fn new(patterns: &[String], managed_subpaths: &[String]) -> Self {
        let mut m = ArtifactMatcher {
            basenames: patterns.iter().cloned().collect(),
            exact_paths: HashMap::with_capacity(managed_subpaths.len()),
            exact_leaf_names: HashSet::with_capacity(managed_subpaths.len()),
        };
        for subpath in managed_subpaths {
            let Some(cleaned) = safe_relative_path(subpath) else {
                continue;
            };
            let display = cleaned.to_string_lossy().replace('\\', "/");
            m.exact_paths.insert(
                cleaned.clone(),
                format!("{MANAGED_ARTIFACT_PATTERN_PREFIX}{display}"),
            );
            if let Some(leaf) = cleaned.file_name() {
                m.exact_leaf_names.insert(leaf.to_string_lossy().into_owned());
            }
        }
        m
    }

    /// `matchDirectory`: `path` is the absolute directory being visited and
    /// `entry_name` its leaf name. Returns the matched artifact pattern.
    pub(crate) fn match_directory(&self, abs_root: &Path, path: &Path, entry_name: &str) -> Option<String> {
        let exact_candidate = self.exact_leaf_names.contains(entry_name);
        let basename_match = self.basenames.contains(entry_name);
        if !exact_candidate && !basename_match {
            return None;
        }
        // Rel and containment validation are only needed for a directory whose
        // leaf could actually match. Most workdir entries avoid this entirely.
        let Ok(rel) = path.strip_prefix(abs_root) else {
            return None;
        };
        let rel = safe_relative_path(rel.to_string_lossy().as_ref())?;
        if let Some(label) = self.exact_paths.get(&rel) {
            return Some(label.clone());
        }
        if basename_match {
            return Some(entry_name.to_string());
        }
        None
    }

    pub(crate) fn managed_subpaths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .exact_paths
            .keys()
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .collect();
        out.sort();
        out
    }
}

/// `safeRelativePath`: trims whitespace, rejects absolute paths (and Windows
/// volume prefixes), rejects ".", ".." and anything escaping upward after
/// cleaning; returns the cleaned path otherwise.
pub(crate) fn safe_relative_path(path: &str) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() || Path::new(trimmed).is_absolute() || has_windows_volume_prefix(trimmed) {
        return None;
    }
    let cleaned = clean_path(trimmed);
    if cleaned.as_os_str() == "." {
        return None;
    }
    if is_parent_escape(&cleaned) {
        return None;
    }
    Some(cleaned)
}

fn has_windows_volume_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Mirrors Go's `filepath.Clean` semantics on slash-separated input (the
/// callers pass either native or slash paths; Rust's `Path` components give
/// the same normalization for both).
fn clean_path(path: &str) -> PathBuf {
    let p = Path::new(path);
    let mut out = PathBuf::new();
    let mut absolute = false;
    for comp in p.components() {
        match comp {
            Component::RootDir => absolute = true,
            Component::CurDir => {}
            Component::ParentDir => {
                // Keep leading .. components (like Clean does).
                match out.file_name() {
                    Some(name) if name != ".." => {
                        out.pop();
                    }
                    _ => out.push(".."),
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    if absolute && !out.starts_with("/") {
        let mut abs = PathBuf::from("/");
        abs.push(out);
        abs
    } else {
        out
    }
}

/// True when the cleaned path escapes upward (".." or "../…").
fn is_parent_escape(cleaned: &Path) -> bool {
    let s = cleaned.to_string_lossy();
    s == ".." || s.starts_with("../") || s.starts_with("..\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher() -> ArtifactMatcher {
        ArtifactMatcher::new(
            &[".sandbox-bin".to_string()],
            &["workdir/.managed".to_string(), "../escape".to_string()],
        )
    }

    #[test]
    fn builds_exact_paths_from_managed_subpaths() {
        let m = matcher();
        assert_eq!(
            m.managed_subpaths(),
            vec!["workdir/.managed".to_string()]
        );
        assert!(m.exact_leaf_names.contains(".managed"));
    }

    #[test]
    fn exact_managed_path_wins_over_basename() {
        let m = matcher();
        let root = Path::new("/root");
        assert_eq!(
            m.match_directory(root, &root.join("workdir/.managed"), ".managed"),
            Some("managed:workdir/.managed".to_string())
        );
    }

    #[test]
    fn plain_basename_match_returns_leaf_name() {
        let m = matcher();
        let root = Path::new("/root");
        assert_eq!(
            m.match_directory(root, &root.join("a/b/.sandbox-bin"), ".sandbox-bin"),
            Some(".sandbox-bin".to_string())
        );
    }

    #[test]
    fn non_matching_entries_return_none() {
        let m = matcher();
        let root = Path::new("/root");
        assert_eq!(
            m.match_directory(root, &root.join("other"), "other"),
            None
        );
    }

    #[test]
    fn safe_relative_path_rejects_escapes_and_absolutes() {
        assert_eq!(safe_relative_path(""), None);
        assert_eq!(safe_relative_path("  "), None);
        assert_eq!(safe_relative_path("/abs"), None);
        assert_eq!(safe_relative_path("C:/x"), None);
        assert_eq!(safe_relative_path("."), None);
        assert_eq!(safe_relative_path(".."), None);
        assert_eq!(safe_relative_path("../x"), None);
        assert_eq!(
            safe_relative_path("./a/../b"),
            Some(PathBuf::from("b"))
        );
    }
}
