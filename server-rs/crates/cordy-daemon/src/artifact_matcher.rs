//! Port of `server/internal/daemon/artifact_matcher.go` (84 lines).
//!
//! Deviations from Go:
//! - `filepath.Clean` → [`crate::repocache::normalize_lexically`] (same
//!   lexical semantics, already used by the gc.rs inline copy).
//! - `os.DirEntry` → the entry's leaf `name: &str` (callers walk with
//!   `walkdir`/`std::fs` and pass `file_name()`), matching the seam shape
//!   already established in gc.rs.
//! - `filepath.VolumeName` is a no-op on unix; the absolute-path check covers
//!   it (Go's own unix behavior).
//!
//! NOTE: gc.rs and diskusage.rs currently carry private inline copies of this
//! logic from earlier lanes; this module is the canonical standalone port.

// S9-integration: canonical matcher consumed by diskusage/gc wiring that
// lands with integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::repocache::normalize_lexically;

/// `managedArtifactPatternPrefix` (artifact_matcher.go:10).
const MANAGED_ARTIFACT_PATTERN_PREFIX: &str = "managed:";

/// `artifactMatcher` (artifact_matcher.go:15–19): combines operator-configured
/// basename matches with exact daemon-managed paths. Exact paths take
/// precedence so a broad basename such as .sandbox-bin cannot double-count a
/// managed directory.
#[derive(Debug, Default, Clone)]
pub(crate) struct ArtifactMatcher {
    basenames: HashSet<String>,
    exact_paths: HashMap<String, String>,
    exact_leaf_names: HashSet<String>,
}

impl ArtifactMatcher {
    /// `newArtifactMatcher` (artifact_matcher.go:21–37).
    pub(crate) fn new(patterns: &[String], managed_subpaths: &[String]) -> Self {
        let mut matcher = ArtifactMatcher {
            basenames: patterns.iter().cloned().collect(),
            exact_paths: HashMap::with_capacity(managed_subpaths.len()),
            exact_leaf_names: HashSet::with_capacity(managed_subpaths.len()),
        };
        for subpath in managed_subpaths {
            let Some(cleaned) = safe_relative_path(subpath) else {
                continue;
            };
            let display = cleaned.replace('\\', "/");
            matcher.exact_paths.insert(
                cleaned.clone(),
                format!("{MANAGED_ARTIFACT_PATTERN_PREFIX}{display}"),
            );
            if let Some(leaf) = Path::new(&cleaned).file_name() {
                matcher
                    .exact_leaf_names
                    .insert(leaf.to_string_lossy().into_owned());
            }
        }
        matcher
    }

    /// `matchDirectory` (artifact_matcher.go:39–63): returns the artifact
    /// label when `path` (a directory under `abs_root` whose leaf name is
    /// `name`) matches either an exact managed subpath or a configured
    /// basename.
    pub(crate) fn match_directory(&self, abs_root: &Path, path: &Path, name: &str) -> Option<String> {
        let exact_candidate = self.exact_leaf_names.contains(name);
        let basename_match = self.basenames.contains(name);
        if !exact_candidate && !basename_match {
            return None;
        }

        // Rel and containment validation are only needed for a directory
        // whose leaf could actually match. Most workdir entries avoid this
        // path entirely.
        let rel = path.strip_prefix(abs_root).ok()?;
        let rel = safe_relative_path(&rel.to_string_lossy())?;
        if let Some(label) = self.exact_paths.get(&rel) {
            return Some(label.clone());
        }
        if basename_match {
            return Some(name.to_string());
        }
        None
    }

    /// `managedSubpaths` (artifact_matcher.go:65–72): slash-normalized,
    /// sorted exact managed paths.
    pub(crate) fn managed_subpaths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .exact_paths
            .keys()
            .map(|rel| rel.replace('\\', "/"))
            .collect();
        out.sort();
        out
    }
}

/// `safeRelativePath` (artifact_matcher.go:74–84): trims, rejects empty /
/// absolute / escaping paths, and returns the lexically-cleaned form.
pub(crate) fn safe_relative_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || Path::new(path).is_absolute() {
        return None;
    }
    let cleaned = normalize_lexically(Path::new(path));
    let cleaned = cleaned.to_string_lossy().into_owned();
    if cleaned == "."
        || cleaned == ".."
        || cleaned.starts_with("../")
        || cleaned.starts_with("..\\")
    {
        return None;
    }
    Some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Semantics of newArtifactMatcher + matchDirectory
    /// (artifact_matcher.go:21–63): an exact managed subpath wins over a
    /// same-named operator basename and carries the `managed:` prefix.
    #[test]
    fn exact_managed_path_takes_precedence() {
        let m = ArtifactMatcher::new(
            &[".sandbox-bin".to_string()],
            &[".sandbox-bin".to_string()],
        );
        let root = Path::new("/tmp/task");
        assert_eq!(
            m.match_directory(root, &root.join(".sandbox-bin"), ".sandbox-bin"),
            Some("managed:.sandbox-bin".to_string())
        );
    }

    /// A basename-only pattern (not a managed subpath) labels by leaf name
    /// (artifact_matcher.go:59–61).
    #[test]
    fn basename_pattern_labels_by_leaf() {
        let m = ArtifactMatcher::new(&["caches".to_string()], &[]);
        let root = Path::new("/tmp/task");
        assert_eq!(
            m.match_directory(root, &root.join("nested/caches"), "caches"),
            Some("caches".to_string())
        );
    }

    /// A leaf that is only a managed-subpath candidate but not at the exact
    /// relative location does not match (artifact_matcher.go:56–62).
    #[test]
    fn wrong_location_managed_leaf_does_not_match() {
        let m = ArtifactMatcher::new(&[], &[".codex/sandbox-bin".to_string()]);
        let root = Path::new("/tmp/task");
        assert_eq!(
            m.match_directory(root, &root.join("sandbox-bin"), "sandbox-bin"),
            None
        );
        assert_eq!(
            m.match_directory(root, &root.join(".codex/sandbox-bin"), "sandbox-bin"),
            Some("managed:.codex/sandbox-bin".to_string())
        );
    }

    /// Non-matching leaves short-circuit before any Rel work
    /// (artifact_matcher.go:40–44).
    #[test]
    fn non_matching_leaf_is_none() {
        let m = ArtifactMatcher::new(&["caches".to_string()], &[".sandbox-bin".to_string()]);
        let root = Path::new("/tmp/task");
        assert_eq!(m.match_directory(root, &root.join("src"), "src"), None);
    }

    /// managedSubpaths returns slash-normalized sorted rels
    /// (artifact_matcher.go:65–72).
    #[test]
    fn managed_subpaths_sorted_and_slashed() {
        let m = ArtifactMatcher::new(
            &[],
            &["b/dir2".to_string(), "a/dir1".to_string()],
        );
        assert_eq!(m.managed_subpaths(), vec!["a/dir1", "b/dir2"]);
    }

    /// safeRelativePath contract (artifact_matcher.go:74–84): trims spaces,
    /// cleans dots, rejects empty/absolute/escaping inputs. Mirrors the
    /// contract test in gc.rs (gc_junction_windows_test.go coverage lives on
    /// the windows lane; these are the platform-neutral cases).
    #[test]
    fn safe_relative_path_contract() {
        assert_eq!(safe_relative_path("  a/b  "), Some("a/b".to_string()));
        assert_eq!(safe_relative_path("./a/./b"), Some("a/b".to_string()));
        assert_eq!(safe_relative_path("a/../b"), Some("b".to_string()));
        assert_eq!(safe_relative_path(""), None);
        assert_eq!(safe_relative_path("   "), None);
        assert_eq!(safe_relative_path("/abs/path"), None);
        assert_eq!(safe_relative_path("."), None);
        assert_eq!(safe_relative_path(".."), None);
        assert_eq!(safe_relative_path("../escape"), None);
    }
}
