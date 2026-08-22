//! Port of `server/internal/daemon/diskusage.go` (lines 1–514).
//!
//! Read-only disk-usage scan of the workspaces root: per-task footprints
//! categorized by `.gc_meta.json` kind, per-workspace aggregates, artifact
//! accounting that mirrors what the GC would actually reclaim (basename
//! patterns + exact daemon-managed subpaths, never inside `.git`), and a
//! separate bare-repo-cache measurement. Purely local — parent statuses are
//! an opt-in second pass.
//!
//! Deviations from Go:
//! - `artifactMatcher` (artifact_matcher.go:1–84) is ported locally below;
//!   S9-integration: consolidate with lane B's artifact_matcher.rs when it
//!   lands. This module must not reference crate::execenv.
//! - `execenv.GCMeta` / `ReadGCMeta` are mirrored locally (gc.rs holds its own
//!   private copy; fields needed here differ). Swap to the shared execenv
//!   module at integration time.
//! - `execenv.ManagedReclaimableArtifactSubpaths` → local stand-in returning
//!   the same single managed path; swap to crate::execenv::reclaimable at
//!   integration.
//! - `dirSize` is reused from [`crate::gc::dir_size`] (same walk semantics:
//!   regular files only, links not counted).
//! - `ResolveParentStatuses` takes `Option<fetcher>` instead of Go's nil-able
//!   func and drops the `context.Context` parameter (cancellation is the
//!   caller's concern); the report is taken as `&mut` instead of a pointer.

// S9-integration: consumed by CLI disk-usage command wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `reposDirName` (gc.go:21).
const REPOS_DIR_NAME: &str = ".repos";

/// `issueGCBatchSize` (gc.go:195): same chunk size the GC loop uses.
const ISSUE_GC_BATCH_SIZE: usize = 500;

/// `DiskUsageKindUnknown` (diskusage.go:138).
pub(crate) const DISK_USAGE_KIND_UNKNOWN: &str = "unknown";

/// `managedArtifactPatternPrefix` (artifact_matcher.go:10).
const MANAGED_ARTIFACT_PATTERN_PREFIX: &str = "managed:";

// ---------------------------------------------------------------------------
// Report types (diskusage.go:22–105).
// ---------------------------------------------------------------------------

/// `TaskDiskUsage` (diskusage.go:22–33).
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct TaskDiskUsage {
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "workspace_short")]
    pub workspace_short: String,
    #[serde(rename = "task_short")]
    pub task_short: String,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "parent_id", skip_serializing_if = "String::is_empty")]
    pub parent_id: String,
    /// Stays empty until [`resolve_parent_statuses`] fills it in.
    #[serde(rename = "parent_status")]
    pub parent_status: String,
    #[serde(rename = "age_seconds")]
    pub age_seconds: i64,
    #[serde(rename = "size_bytes")]
    pub size_bytes: i64,
    #[serde(rename = "artifact_size_bytes")]
    pub artifact_size_bytes: i64,
}

/// `WorkspaceDiskUsage` (diskusage.go:40–48).
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct WorkspaceDiskUsage {
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "workspace_short")]
    pub workspace_short: String,
    #[serde(rename = "task_count")]
    pub task_count: i64,
    #[serde(rename = "size_bytes")]
    pub size_bytes: i64,
    #[serde(rename = "artifact_size_bytes")]
    pub artifact_size_bytes: i64,
    /// Fraction (0..1) of size_bytes the GC artifact cleanup could reclaim.
    #[serde(rename = "artifact_ratio")]
    pub artifact_ratio: f64,
    #[serde(rename = "oldest_age_seconds")]
    pub oldest_age_seconds: i64,
}

/// `DiskUsageReport` (diskusage.go:53–72). Total* fields always reflect the
/// entire scan, never a post-`--top` truncated view.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct DiskUsageReport {
    #[serde(rename = "workspaces_root")]
    pub workspaces_root: String,
    #[serde(rename = "generated_at")]
    pub generated_at: DateTime<Utc>,
    #[serde(rename = "artifact_patterns")]
    pub artifact_patterns: Vec<String>,
    #[serde(rename = "managed_artifact_subpaths")]
    pub managed_artifact_subpaths: Vec<String>,
    #[serde(rename = "tasks")]
    pub tasks: Vec<TaskDiskUsage>,
    #[serde(rename = "workspaces")]
    pub workspaces: Vec<WorkspaceDiskUsage>,
    #[serde(rename = "total_task_count")]
    pub total_task_count: usize,
    #[serde(rename = "total_workspace_count")]
    pub total_workspace_count: usize,
    #[serde(rename = "total_size_bytes")]
    pub total_size_bytes: i64,
    #[serde(rename = "total_artifact_size_bytes")]
    pub total_artifact_size_bytes: i64,
    #[serde(rename = "total_artifact_ratio")]
    pub total_artifact_ratio: f64,
    /// Bare-repo cache (.repos) footprint — sibling of task dirs, reported
    /// separately and excluded from Total* to avoid double-counting.
    #[serde(rename = "repo_cache_size_bytes")]
    pub repo_cache_size_bytes: i64,
    #[serde(rename = "repo_cache_count")]
    pub repo_cache_count: usize,
}

/// `DiskUsageRoot` (diskusage.go:78–81).
#[derive(Debug, Clone, Default)]
pub(crate) struct DiskUsageRoot {
    pub profile: String,
    pub root: String,
}

/// `RootDiskUsage` (diskusage.go:84–87).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RootDiskUsage {
    #[serde(rename = "profile")]
    pub profile: String,
    #[serde(rename = "report")]
    pub report: DiskUsageReport,
}

/// `AggregateDiskUsageReport` (diskusage.go:93–105).
#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct AggregateDiskUsageReport {
    #[serde(rename = "generated_at")]
    pub generated_at: DateTime<Utc>,
    #[serde(rename = "artifact_patterns")]
    pub artifact_patterns: Vec<String>,
    #[serde(rename = "managed_artifact_subpaths")]
    pub managed_artifact_subpaths: Vec<String>,
    #[serde(rename = "roots")]
    pub roots: Vec<RootDiskUsage>,
    #[serde(rename = "total_task_count")]
    pub total_task_count: usize,
    #[serde(rename = "total_workspace_count")]
    pub total_workspace_count: usize,
    #[serde(rename = "total_size_bytes")]
    pub total_size_bytes: i64,
    #[serde(rename = "total_artifact_size_bytes")]
    pub total_artifact_size_bytes: i64,
    #[serde(rename = "total_artifact_ratio")]
    pub total_artifact_ratio: f64,
    #[serde(rename = "total_repo_cache_size_bytes")]
    pub total_repo_cache_size_bytes: i64,
    #[serde(rename = "total_repo_cache_count")]
    pub total_repo_cache_count: usize,
}

// ---------------------------------------------------------------------------
// Local seams (see module header deviations).
// ---------------------------------------------------------------------------

/// `execenv.ManagedReclaimableArtifactSubpaths`
/// (execenv/reclaimable.go): daemon-owned regenerable directories inside a
/// task env root, matched as exact relative paths rather than basenames.
fn managed_reclaimable_artifact_subpaths() -> Vec<String> {
    // S9-integration: swap for crate::execenv::reclaimable::
    // managed_reclaimable_artifact_subpaths() at integration time.
    vec!["codex-home/.sandbox-bin".to_string()]
}

/// `execenv.GCMetaKind` string values (execenv.go:947–953).
mod gc_kind {
    pub const ISSUE: &str = "issue";
    pub const CHAT: &str = "chat";
    pub const AUTOPILOT_RUN: &str = "autopilot_run";
    pub const QUICK_CREATE: &str = "quick_create";
}

/// Minimal mirror of `execenv.GCMeta` (execenv.go:963–984) covering the
/// fields this module reads.
#[derive(Debug, Clone, Default, Deserialize)]
struct DiskGcMeta {
    #[serde(default, rename = "kind")]
    kind_raw: String,
    #[serde(default, rename = "issue_id")]
    issue_id: String,
    #[serde(default, rename = "chat_session_id")]
    chat_session_id: String,
    #[serde(default, rename = "autopilot_run_id")]
    autopilot_run_id: String,
    #[serde(default, rename = "task_id")]
    task_id: String,
    #[serde(default, rename = "completed_at")]
    completed_at: Option<DateTime<Utc>>,
}

impl DiskGcMeta {
    fn kind(&self) -> &str {
        match self.kind_raw.as_str() {
            "" | gc_kind::ISSUE => gc_kind::ISSUE,
            other => other,
        }
    }
}

/// `execenv.ReadGCMeta` (execenv.go:1008–1023), returning None when absent or
/// unreadable (Go callers treat any error as "no meta").
fn read_gc_meta(env_root: &Path) -> Option<DiskGcMeta> {
    let data = std::fs::read(env_root.join(".gc_meta.json")).ok()?;
    let mut meta: DiskGcMeta = serde_json::from_slice(&data).ok()?;
    if meta.kind_raw.is_empty() {
        meta.kind_raw = gc_kind::ISSUE.to_string();
    }
    Some(meta)
}

/// `gcMetaFileAge` (gc.go:598–604): age of the meta file itself.
fn gc_meta_file_age(task_dir: &Path) -> Option<chrono::Duration> {
    let info = std::fs::metadata(task_dir.join(".gc_meta.json")).ok()?;
    let modified = info.modified().ok()?;
    Some(Utc::now().signed_duration_since(DateTime::<Utc>::from(modified)))
}

// ---------------------------------------------------------------------------
// Artifact matcher (artifact_matcher.go:10–84).
// ---------------------------------------------------------------------------

/// `safeRelativePath` (artifact_matcher.go:74–84): trim, reject empty /
/// absolute / parent-escaping paths, return the cleaned form.
fn safe_relative_path(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || Path::new(trimmed).is_absolute() {
        return None;
    }
    // filepath.Clean equivalent over slash-separated components: drop empty
    // and "." segments, resolve ".." lexically, reject escaping results.
    let mut parts: Vec<&str> = Vec::new();
    for seg in trimmed.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last() == Some(&"..") || parts.is_empty() {
                    return None; // escapes the root (covers cleaned == "..")
                }
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None; // cleaned == "."
    }
    Some(parts.join("/"))
}

/// `artifactMatcher` (artifact_matcher.go:15–19): operator-configured basename
/// matches plus exact daemon-managed paths; exact wins so a broad basename
/// such as .sandbox-bin cannot double-count a managed directory.
#[derive(Debug, Clone, Default)]
struct ArtifactMatcher {
    basenames: HashSet<String>,
    exact_paths: HashMap<String, String>,
    exact_leaf_names: HashSet<String>,
}

impl ArtifactMatcher {
    /// `newArtifactMatcher` (artifact_matcher.go:21–37).
    fn new(patterns: &[String], managed_subpaths: &[String]) -> Self {
        let mut m = Self {
            basenames: build_pattern_set(patterns),
            exact_paths: HashMap::with_capacity(managed_subpaths.len()),
            exact_leaf_names: HashSet::with_capacity(managed_subpaths.len()),
        };
        for subpath in managed_subpaths {
            let Some(cleaned) = safe_relative_path(subpath) else {
                continue;
            };
            let display = cleaned.replace('\\', "/");
            m.exact_paths.insert(
                cleaned.clone(),
                format!("{MANAGED_ARTIFACT_PATTERN_PREFIX}{display}"),
            );
            if let Some(leaf) = cleaned.rsplit('/').next() {
                m.exact_leaf_names.insert(leaf.to_string());
            }
        }
        m
    }

    /// `matchDirectory` (artifact_matcher.go:39–63): returns the matched label.
    fn match_directory(&self, abs_root: &Path, path: &Path, name: &str) -> Option<String> {
        let exact_candidate = self.exact_leaf_names.contains(name);
        let basename_match = self.basenames.contains(name);
        if !exact_candidate && !basename_match {
            return None;
        }
        // Rel + containment validation only for leaves that could match.
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

    /// `managedSubpaths` (artifact_matcher.go:65–72).
    fn managed_subpaths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .exact_paths
            .keys()
            .map(|r| r.replace('\\', "/"))
            .collect();
        out.sort();
        out
    }
}

/// `buildPatternSet` (diskusage.go:285–295): trim, drop empty and
/// separator-bearing patterns.
fn build_pattern_set(patterns: &[String]) -> HashSet<String> {
    patterns
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && !p.contains('/') && !p.contains('\\'))
        .map(|p| p.to_string())
        .collect()
}

/// `sortedKeys` (diskusage.go:297–304).
fn sorted_keys(set: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = set.iter().cloned().collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Scan (diskusage.go:107–305).
// ---------------------------------------------------------------------------

/// `ScanDiskUsageRoots` (diskusage.go:112–133): scan every root in order and
/// combine. A missing root yields an empty per-root report, not an error; a
/// genuinely unreadable root aborts the whole scan.
pub(crate) fn scan_disk_usage_roots(
    roots: &[DiskUsageRoot],
    artifact_patterns: &[String],
) -> anyhow::Result<AggregateDiskUsageReport> {
    let matcher = ArtifactMatcher::new(artifact_patterns, &managed_reclaimable_artifact_subpaths());
    let mut agg = AggregateDiskUsageReport {
        generated_at: Utc::now(),
        artifact_patterns: sorted_keys(&matcher.basenames),
        managed_artifact_subpaths: matcher.managed_subpaths(),
        ..Default::default()
    };

    for r in roots {
        let report = scan_disk_usage(&r.root, artifact_patterns)?;
        agg.total_task_count += report.total_task_count;
        agg.total_workspace_count += report.total_workspace_count;
        agg.total_size_bytes += report.total_size_bytes;
        agg.total_artifact_size_bytes += report.total_artifact_size_bytes;
        agg.total_repo_cache_size_bytes += report.repo_cache_size_bytes;
        agg.total_repo_cache_count += report.repo_cache_count;
        agg.roots.push(RootDiskUsage {
            profile: r.profile.clone(),
            report,
        });
    }
    agg.total_artifact_ratio = ratio(agg.total_artifact_size_bytes, agg.total_size_bytes);
    Ok(agg)
}

/// `ScanDiskUsage` (diskusage.go:151–245): walk `workspaces_root`. Read-only,
/// never follows symlinks, counts only regular files. Missing roots return an
/// empty report, not an error.
pub(crate) fn scan_disk_usage(
    workspaces_root: &str,
    artifact_patterns: &[String],
) -> anyhow::Result<DiskUsageReport> {
    let mut report = DiskUsageReport {
        workspaces_root: workspaces_root.to_string(),
        generated_at: Utc::now(),
        ..Default::default()
    };
    if workspaces_root.is_empty() {
        anyhow::bail!("disk-usage: workspaces root is required");
    }

    let matcher = ArtifactMatcher::new(artifact_patterns, &managed_reclaimable_artifact_subpaths());
    report.artifact_patterns = sorted_keys(&matcher.basenames);
    report.managed_artifact_subpaths = matcher.managed_subpaths();

    let ws_entries = match std::fs::read_dir(workspaces_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(e) => return Err(anyhow::Error::new(e).context("disk-usage: read workspaces root")),
    };

    let mut ws_agg: HashMap<String, WorkspaceDiskUsage> = HashMap::new();

    for ws_entry in ws_entries.flatten() {
        let is_dir = ws_entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let name = ws_entry.file_name().to_string_lossy().into_owned();
        // The bare-repo cache is measured separately, not skipped outright.
        if name == REPOS_DIR_NAME {
            let (size, count) = repo_cache_size(&Path::new(workspaces_root).join(&name));
            report.repo_cache_size_bytes = size;
            report.repo_cache_count = count;
            continue;
        }
        // Other dot-directories are daemon-internal caches, never workspaces.
        if name.starts_with('.') {
            continue;
        }
        let ws_id = name;
        let ws_dir = Path::new(workspaces_root).join(&ws_id);
        let Ok(task_entries) = std::fs::read_dir(&ws_dir) else {
            continue;
        };
        for t in task_entries.flatten() {
            let t_is_dir = t.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if !t_is_dir {
                continue;
            }
            let task_short = t.file_name().to_string_lossy().into_owned();
            let task_dir = ws_dir.join(&task_short);
            let usage = build_task_usage(&task_dir, &ws_id, &task_short, &matcher);

            report.total_size_bytes += usage.size_bytes;
            report.total_artifact_size_bytes += usage.artifact_size_bytes;

            let ws = ws_agg
                .entry(ws_id.clone())
                .or_insert_with(|| WorkspaceDiskUsage {
                    workspace_id: ws_id.clone(),
                    workspace_short: short_id(&ws_id),
                    ..Default::default()
                });
            ws.task_count += 1;
            ws.size_bytes += usage.size_bytes;
            ws.artifact_size_bytes += usage.artifact_size_bytes;
            if usage.age_seconds > ws.oldest_age_seconds {
                ws.oldest_age_seconds = usage.age_seconds;
            }

            report.tasks.push(usage);
        }
    }

    report.tasks.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    let mut workspaces: Vec<WorkspaceDiskUsage> = ws_agg.into_values().collect();
    for ws in &mut workspaces {
        ws.artifact_ratio = ratio(ws.artifact_size_bytes, ws.size_bytes);
    }
    workspaces.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    report.total_task_count = report.tasks.len();
    report.total_workspace_count = workspaces.len();
    report.workspaces = workspaces;
    report.total_artifact_ratio = ratio(report.total_artifact_size_bytes, report.total_size_bytes);

    Ok(report)
}

/// `repoCacheSize` (diskusage.go:250–273): measure the bare-repo cache; the
/// count is the number of second-level directories — the unit the GC evicts.
fn repo_cache_size(repos_root: &Path) -> (i64, usize) {
    let Ok(ws_entries) = std::fs::read_dir(repos_root) else {
        return (0, 0);
    };
    let mut size_bytes = 0i64;
    let mut repo_count = 0usize;
    for ws_entry in ws_entries.flatten() {
        if !ws_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let ws_dir = repos_root.join(ws_entry.file_name());
        let Ok(repo_entries) = std::fs::read_dir(&ws_dir) else {
            continue;
        };
        for repo_entry in repo_entries.flatten() {
            if !repo_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            repo_count += 1;
            size_bytes += crate::gc::dir_size(&ws_dir.join(repo_entry.file_name()));
        }
    }
    (size_bytes, repo_count)
}

/// `ratio` (diskusage.go:278–283): 0/0 → 0, never NaN.
fn ratio(numerator: i64, denominator: i64) -> f64 {
    if denominator <= 0 {
        return 0.0;
    }
    numerator as f64 / denominator as f64
}

/// `buildTaskUsage` (diskusage.go:306–337).
fn build_task_usage(
    task_dir: &Path,
    ws_id: &str,
    task_short: &str,
    matcher: &ArtifactMatcher,
) -> TaskDiskUsage {
    let mut usage = TaskDiskUsage {
        workspace_id: ws_id.to_string(),
        workspace_short: short_id(ws_id),
        task_short: task_short.to_string(),
        path: task_dir.to_string_lossy().into_owned(),
        kind: DISK_USAGE_KIND_UNKNOWN.to_string(),
        ..Default::default()
    };

    let mut meta_present = false;
    if let Some(meta) = read_gc_meta(task_dir) {
        meta_present = true;
        usage.kind = meta.kind().to_string();
        usage.parent_id = parent_id_for_meta(&meta);
        if let Some(completed_at) = meta.completed_at {
            usage.age_seconds = Utc::now().signed_duration_since(completed_at).num_seconds();
        } else if let Some(age) = gc_meta_file_age(task_dir) {
            usage.age_seconds = age.num_seconds();
        }
    }
    // No readable metadata → taskDir mtime, like orphanByMTime. Legacy
    // readable metadata without completed_at keeps its file-mtime age above.
    if usage.age_seconds <= 0 && !meta_present {
        if let Ok(info) = std::fs::metadata(task_dir) {
            if let Ok(modified) = info.modified() {
                usage.age_seconds = Utc::now()
                    .signed_duration_since(DateTime::<Utc>::from(modified))
                    .num_seconds();
            }
        }
    }

    let (size_bytes, artifact_size_bytes) = task_size(task_dir, matcher);
    usage.size_bytes = size_bytes;
    usage.artifact_size_bytes = artifact_size_bytes;
    usage
}

/// `parentIDForMeta` (diskusage.go:342–355): GCMeta is a discriminated union
/// keyed on Kind — only the field matching Kind is meaningful.
fn parent_id_for_meta(meta: &DiskGcMeta) -> String {
    match meta.kind() {
        gc_kind::ISSUE => meta.issue_id.trim().to_string(),
        gc_kind::CHAT => meta.chat_session_id.trim().to_string(),
        gc_kind::AUTOPILOT_RUN => meta.autopilot_run_id.trim().to_string(),
        gc_kind::QUICK_CREATE => meta.task_id.trim().to_string(),
        _ => String::new(),
    }
}

/// `taskSize` (diskusage.go:449–503): walk taskDir returning
/// `(totalBytes, artifactBytes)`. Never follows symlinks, counts only regular
/// files. A directory matched by the matcher is an artifact subtree — added to
/// both totals, not descended into. A `.git` subtree counts toward total but
/// never toward artifacts, and is never descended into.
fn task_size(task_dir: &Path, matcher: &ArtifactMatcher) -> (i64, i64) {
    if task_dir.as_os_str().is_empty() {
        return (0, 0);
    }
    let abs_root = absolute(task_dir);
    let mut total_bytes = 0i64;
    let mut artifact_bytes = 0i64;
    walk_task_dir(
        &abs_root,
        &abs_root,
        matcher,
        &mut total_bytes,
        &mut artifact_bytes,
    );
    (total_bytes, artifact_bytes)
}

fn walk_task_dir(
    dir: &Path,
    abs_root: &Path,
    matcher: &ArtifactMatcher,
    total_bytes: &mut i64,
    artifact_bytes: &mut i64,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        // Links: never followed, never counted (symlinked files would
        // otherwise sum their targets' bytes via metadata on some platforms).
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ft.is_dir() {
            let name = entry.file_name();
            if name == ".git" {
                *total_bytes += crate::gc::dir_size(&path);
                continue;
            }
            if matcher
                .match_directory(abs_root, &path, &name.to_string_lossy())
                .is_some()
            {
                let size = crate::gc::dir_size(&path);
                *total_bytes += size;
                *artifact_bytes += size;
                continue;
            }
            walk_task_dir(&path, abs_root, matcher, total_bytes, artifact_bytes);
            continue;
        }
        if let Ok(info) = entry.metadata() {
            if info.is_file() {
                *total_bytes += info.len() as i64;
            }
        }
    }
}

/// `filepath.Abs` equivalent.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// `ShortID` (diskusage.go:508–514): first 8 chars (dashes stripped) of a
/// UUID, raw input when shorter.
pub(crate) fn short_id(id: &str) -> String {
    let s = id.replace('-', "");
    if s.len() > 8 {
        s[..8].to_string()
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Parent-status resolution (diskusage.go:361–434).
// ---------------------------------------------------------------------------

/// `ParentStatusFetcher` (diskusage.go:361): resolves a batch of issue ids in
/// one workspace to their current status. Ids the server does not return must
/// be omitted, so callers can tell "unresolved" from a real status.
pub(crate) trait ParentStatusFetcher {
    fn fetch(
        &mut self,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, String>>;
}

impl<F> ParentStatusFetcher for F
where
    F: FnMut(&str, &[String]) -> anyhow::Result<HashMap<String, String>>,
{
    fn fetch(
        &mut self,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, String>> {
        self(workspace_id, issue_ids)
    }
}

/// `ResolveParentStatuses` (diskusage.go:376–434): fill ParentStatus on every
/// issue-kind task. Best-effort by design — one failing workspace leaves its
/// tasks unresolved but does not stop the others; the first error surfaces.
/// `fetch = None` is a no-op (offline use).
pub(crate) fn resolve_parent_statuses(
    report: &mut DiskUsageReport,
    mut fetch: Option<&mut dyn ParentStatusFetcher>,
) -> anyhow::Result<()> {
    let Some(fetch) = fetch.as_deref_mut() else {
        return Ok(());
    };

    let mut ids_by_workspace: HashMap<String, Vec<String>> = HashMap::new();
    let mut seen: HashMap<String, HashSet<String>> = HashMap::new();
    for task in &report.tasks {
        if task.kind != gc_kind::ISSUE || task.parent_id.is_empty() {
            continue;
        }
        // Several task dirs can share one issue — de-duplicate before asking.
        if !seen
            .entry(task.workspace_id.clone())
            .or_default()
            .insert(task.parent_id.clone())
        {
            continue;
        }
        ids_by_workspace
            .entry(task.workspace_id.clone())
            .or_default()
            .push(task.parent_id.clone());
    }
    if ids_by_workspace.is_empty() {
        return Ok(());
    }

    let mut first_err: Option<anyhow::Error> = None;
    let mut statuses: HashMap<String, HashMap<String, String>> =
        HashMap::with_capacity(ids_by_workspace.len());
    for (workspace_id, ids) in &ids_by_workspace {
        let mut resolved: HashMap<String, String> = HashMap::with_capacity(ids.len());
        // Same chunk size the GC loop uses, so one oversized root cannot trip
        // the server's batch cap.
        for chunk in ids.chunks(ISSUE_GC_BATCH_SIZE) {
            match fetch.fetch(workspace_id, chunk) {
                Ok(result) => resolved.extend(result),
                Err(err) => {
                    first_err.get_or_insert(err);
                }
            }
        }
        statuses.insert(workspace_id.clone(), resolved);
    }

    for task in &mut report.tasks {
        if task.kind != gc_kind::ISSUE || task.parent_id.is_empty() {
            continue;
        }
        if let Some(status) = statuses
            .get(&task.workspace_id)
            .and_then(|m| m.get(&task.parent_id))
        {
            task.parent_status = status.clone();
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Tests (diskusage_test.go pure-logic cases).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, size: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; size]).unwrap();
    }

    /// os.Chtimes equivalent via utimensat (unix tests only).
    #[cfg(unix)]
    fn backdate_mtime(path: &Path, seconds_ago: i64) {
        use std::ffi::CString;
        let c = CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let target = now - seconds_ago;
        let times = [
            libc::timespec {
                tv_sec: target,
                tv_nsec: 0,
            },
            libc::timespec {
                tv_sec: target,
                tv_nsec: 0,
            },
        ];
        unsafe {
            assert_eq!(
                libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0),
                0
            );
        }
    }

    fn must_write_meta(task_dir: &Path, meta: &serde_json::Value) {
        std::fs::create_dir_all(task_dir).unwrap();
        std::fs::write(
            task_dir.join(".gc_meta.json"),
            serde_json::to_vec(meta).unwrap(),
        )
        .unwrap();
    }

    /// TestScanDiskUsage_AggregatesAndCategorizes (diskusage_test.go:35–208).
    #[test]
    fn aggregates_and_categorizes() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let ws_a = "11111111-1111-1111-1111-111111111111";
        let ws_b = "22222222-2222-2222-2222-222222222222";

        let task_a1 = root.join(ws_a).join("aaaaaaaa");
        write_file(&task_a1.join("workdir/main.go"), 1000);
        write_file(&task_a1.join("workdir/node_modules/dep/index.js"), 4000);
        must_write_meta(
            &task_a1,
            &serde_json::json!({
                "kind": "issue", "issue_id": "issue-1", "workspace_id": ws_a,
                "completed_at": (Utc::now() - chrono::Duration::hours(3)).to_rfc3339()
            }),
        );

        let task_a2 = root.join(ws_a).join("bbbbbbbb");
        write_file(&task_a2.join("workdir/notes.md"), 500);
        must_write_meta(
            &task_a2,
            &serde_json::json!({
                "kind": "chat", "chat_session_id": "chat-1", "workspace_id": ws_a,
                "completed_at": (Utc::now() - chrono::Duration::hours(1)).to_rfc3339()
            }),
        );

        let task_b1 = root.join(ws_b).join("cccccccc");
        write_file(&task_b1.join("workdir/result.txt"), 2000);
        // Backdate the dir mtime so the fallback produces a measurable age.
        backdate_mtime(&task_b1, 2 * 3600);

        let report = scan_disk_usage(
            root.to_str().unwrap(),
            &["node_modules".into(), ".next".into(), ".turbo".into()],
        )
        .unwrap();

        assert_eq!(report.tasks.len(), 3);

        let by_short: HashMap<&str, &TaskDiskUsage> = report
            .tasks
            .iter()
            .map(|t| (t.task_short.as_str(), t))
            .collect();

        let a1 = by_short["aaaaaaaa"];
        assert_eq!(a1.kind, "issue");
        // Size includes main.go (1000) + node_modules (4000) + .gc_meta.json.
        assert!(
            (5000..=6024).contains(&a1.size_bytes),
            "task a1 size = {}",
            a1.size_bytes
        );
        assert_eq!(a1.artifact_size_bytes, 4000);
        assert!(a1.age_seconds >= 60, "age_seconds = {}", a1.age_seconds);
        assert_eq!(a1.workspace_short, short_id(ws_a));

        let a2 = by_short["bbbbbbbb"];
        assert_eq!(a2.kind, "chat");
        assert!((500..=1524).contains(&a2.size_bytes));
        assert_eq!(a2.artifact_size_bytes, 0);

        let b1 = by_short["cccccccc"];
        assert_eq!(b1.kind, DISK_USAGE_KIND_UNKNOWN);
        assert_eq!(b1.size_bytes, 2000, "no meta file");
        assert!(b1.age_seconds >= 60, "mtime backdated 2h");

        assert_eq!(
            report.total_size_bytes,
            a1.size_bytes + a2.size_bytes + b1.size_bytes
        );
        assert_eq!(report.total_artifact_size_bytes, 4000);

        let ws_by_id: HashMap<&str, &WorkspaceDiskUsage> = report
            .workspaces
            .iter()
            .map(|w| (w.workspace_id.as_str(), w))
            .collect();
        assert_eq!(ws_by_id[ws_a].size_bytes, a1.size_bytes + a2.size_bytes);
        assert_eq!(ws_by_id[ws_a].artifact_size_bytes, 4000);
        assert_eq!(ws_by_id[ws_a].task_count, 2);
        assert_eq!(ws_by_id[ws_b].size_bytes, 2000);

        let want_a_ratio = 4000.0 / (a1.size_bytes + a2.size_bytes) as f64;
        assert!(
            (ws_by_id[ws_a].artifact_ratio - want_a_ratio).abs() < 0.005,
            "workspace A artifact_ratio = {}",
            ws_by_id[ws_a].artifact_ratio
        );
        assert_eq!(ws_by_id[ws_b].artifact_ratio, 0.0, "no NaN");

        assert_eq!(report.total_task_count, 3);
        assert_eq!(report.total_workspace_count, 2);
        assert!(report.total_artifact_ratio > 0.0 && report.total_artifact_ratio <= 1.0);

        for i in 1..report.tasks.len() {
            assert!(
                report.tasks[i - 1].size_bytes >= report.tasks[i].size_bytes,
                "tasks not sorted by size desc at idx {i}"
            );
        }

        // JSON round-trip guards the field names.
        let raw = serde_json::to_string(&report).unwrap();
        for want in [
            "\"kind\"",
            "\"parent_status\"",
            "\"age_seconds\"",
            "\"size_bytes\"",
            "\"artifact_size_bytes\"",
            "\"workspace_id\"",
            "\"task_short\"",
            "\"artifact_ratio\"",
            "\"managed_artifact_subpaths\"",
            "\"total_task_count\"",
            "\"total_workspace_count\"",
            "\"total_artifact_ratio\"",
        ] {
            assert!(raw.contains(want), "JSON missing required field {want}");
        }
    }

    /// TestScanDiskUsage_ManagedCodexSandboxIsExactAndDeduplicated
    /// (diskusage_test.go:210–240).
    #[test]
    fn managed_codex_sandbox_is_exact_and_deduplicated() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root
            .path()
            .join("mmmmmmmm-mmmm-mmmm-mmmm-mmmmmmmmmmmm")
            .join("tttttttt");
        write_file(&task_dir.join("codex-home/.sandbox-bin/codex.exe"), 300);
        write_file(&task_dir.join("workdir/repo/.sandbox-bin/cache"), 400);

        let report = scan_disk_usage(root.path().to_str().unwrap(), &[]).unwrap();
        assert_eq!(report.tasks[0].size_bytes, 700);
        assert_eq!(
            report.tasks[0].artifact_size_bytes, 300,
            "exact managed only"
        );
        assert_eq!(
            report.managed_artifact_subpaths.join(","),
            "codex-home/.sandbox-bin"
        );

        let report =
            scan_disk_usage(root.path().to_str().unwrap(), &[".sandbox-bin".into()]).unwrap();
        assert_eq!(
            report.tasks[0].artifact_size_bytes, 700,
            "no double counting"
        );
    }

    /// TestScanDiskUsage_LegacyMetaAgeUsesMetaFileMTime
    /// (diskusage_test.go:242–268).
    #[test]
    fn legacy_meta_age_uses_meta_file_mtime() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root
            .path()
            .join("llllllll-llll-llll-llll-llllllllllll")
            .join("tttttttt");
        write_file(&task_dir.join("workdir/main.go"), 10);
        must_write_meta(
            &task_dir,
            &serde_json::json!({"kind": "issue", "issue_id": "issue-legacy"}),
        );
        backdate_mtime(&task_dir, 30 * 24 * 3600);
        backdate_mtime(&task_dir.join(".gc_meta.json"), 2 * 3600);

        let report = scan_disk_usage(root.path().to_str().unwrap(), &[]).unwrap();
        assert!(
            (3600..=3 * 3600).contains(&report.tasks[0].age_seconds),
            "age_seconds={} want near 2h from meta file, not stale root",
            report.tasks[0].age_seconds
        );
    }

    /// TestScanDiskUsage_EmptyWorkspaceArtifactRatio (diskusage_test.go:274–297).
    #[test]
    fn empty_workspace_artifact_ratio() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root
            .path()
            .join("00000000-0000-0000-0000-000000000000")
            .join("tttttttt");
        std::fs::create_dir_all(task_dir.join("workdir")).unwrap();

        let report =
            scan_disk_usage(root.path().to_str().unwrap(), &["node_modules".into()]).unwrap();
        assert_eq!(report.workspaces.len(), 1);
        assert_eq!(report.workspaces[0].artifact_ratio, 0.0, "no NaN");
        assert_eq!(report.total_artifact_ratio, 0.0, "no NaN");
    }

    /// TestScanDiskUsage_CountsGitButNeverAsArtifact (diskusage_test.go:304–331).
    #[test]
    fn counts_git_but_never_as_artifact() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root
            .path()
            .join("wwwwwwww-wwww-wwww-wwww-wwwwwwwwwwww")
            .join("tttttttt");
        write_file(&task_dir.join("workdir/.git/objects/pack"), 9999);
        write_file(&task_dir.join("workdir/.git/node_modules/x"), 5555);
        write_file(&task_dir.join("workdir/main.go"), 100);

        let report =
            scan_disk_usage(root.path().to_str().unwrap(), &["node_modules".into()]).unwrap();
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(report.tasks[0].size_bytes, 100 + 9999 + 5555);
        assert_eq!(report.tasks[0].artifact_size_bytes, 0);
    }

    /// TestScanDiskUsage_DoesNotFollowSymlinks (diskusage_test.go:337–374).
    #[test]
    #[cfg(unix)]
    fn does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_file(&outside.path().join("huge.bin"), 10000);

        let task_dir = root
            .path()
            .join("ssssssss-ssss-ssss-ssss-ssssssssssss")
            .join("tttttttt");
        write_file(&task_dir.join("workdir/main.go"), 100);
        std::os::unix::fs::symlink(outside.path(), task_dir.join("workdir/node_modules")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("huge.bin"),
            task_dir.join("workdir/big-link"),
        )
        .unwrap();

        let report =
            scan_disk_usage(root.path().to_str().unwrap(), &["node_modules".into()]).unwrap();
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(
            report.tasks[0].size_bytes, 100,
            "only main.go; symlinks ignored"
        );
        assert_eq!(report.tasks[0].artifact_size_bytes, 0);
    }

    /// TestScanDiskUsage_MissingRoot (diskusage_test.go:378–387).
    #[test]
    fn missing_root_returns_empty_report() {
        let missing = tempfile::tempdir().unwrap().path().join("does-not-exist");
        let report = scan_disk_usage(missing.to_str().unwrap(), &[]).unwrap();
        assert!(report.tasks.is_empty());
        assert!(report.workspaces.is_empty());
    }

    /// TestScanDiskUsage_RejectsPatternsWithSeparators (diskusage_test.go:392–410).
    #[test]
    fn rejects_patterns_with_separators() {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root
            .path()
            .join("rrrrrrrr-rrrr-rrrr-rrrr-rrrrrrrrrrrr")
            .join("tttttttt");
        write_file(&task_dir.join("workdir/node_modules/x"), 1000);

        let report = scan_disk_usage(
            root.path().to_str().unwrap(),
            &["workdir/node_modules".into(), "../etc".into()],
        )
        .unwrap();
        assert_eq!(report.tasks[0].artifact_size_bytes, 0);
        assert!(report.artifact_patterns.is_empty(), "all dropped");
    }

    /// TestScanDiskUsageRoots_SumsAcrossRoots (diskusage_test.go:416–458).
    #[test]
    fn roots_sums_across_roots() {
        let root_a = tempfile::tempdir().unwrap();
        write_file(
            &root_a
                .path()
                .join("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
                .join("t1")
                .join("workdir/main.go"),
            100,
        );
        let root_b = tempfile::tempdir().unwrap();
        write_file(
            &root_b
                .path()
                .join("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
                .join("t1")
                .join("workdir/big"),
            300,
        );
        write_file(
            &root_b
                .path()
                .join("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
                .join("t2")
                .join("workdir/main.go"),
            50,
        );
        let missing = tempfile::tempdir().unwrap().path().join("never-ran");

        let agg = scan_disk_usage_roots(
            &[
                DiskUsageRoot {
                    profile: String::new(),
                    root: root_a.path().to_string_lossy().into_owned(),
                },
                DiskUsageRoot {
                    profile: "desktop-host".into(),
                    root: root_b.path().to_string_lossy().into_owned(),
                },
                DiskUsageRoot {
                    profile: "never-ran".into(),
                    root: missing.to_string_lossy().into_owned(),
                },
            ],
            &["node_modules".into()],
        )
        .unwrap();

        assert_eq!(agg.roots.len(), 3, "missing root still listed, empty");
        assert_eq!(agg.roots[0].profile, "");
        assert_eq!(agg.roots[1].profile, "desktop-host");
        assert_eq!(agg.roots[2].report.total_task_count, 0);
        assert_eq!(agg.total_task_count, 3);
        assert_eq!(agg.total_size_bytes, 450);
        assert_eq!(agg.total_workspace_count, 2);
        assert_eq!(
            agg.managed_artifact_subpaths.join(","),
            "codex-home/.sandbox-bin"
        );
    }

    fn issue_task(ws_id: &str, task_short: &str, issue_id: &str) -> TaskDiskUsage {
        TaskDiskUsage {
            workspace_id: ws_id.to_string(),
            task_short: task_short.to_string(),
            kind: gc_kind::ISSUE.to_string(),
            parent_id: issue_id.to_string(),
            ..Default::default()
        }
    }

    /// TestResolveParentStatuses_FillsIssueTasks (diskusage_test.go:487–537).
    #[test]
    fn resolve_parent_statuses_fills_issue_tasks() {
        let ws_a = "11111111-1111-1111-1111-111111111111";
        let ws_b = "22222222-2222-2222-2222-222222222222";
        let mut report = DiskUsageReport {
            tasks: vec![
                issue_task(ws_a, "aaaa1111", "issue-1"),
                issue_task(ws_a, "aaaa2222", "issue-1"),
                issue_task(ws_a, "aaaa3333", "issue-2"),
                issue_task(ws_b, "bbbb1111", "issue-3"),
                TaskDiskUsage {
                    workspace_id: ws_a.to_string(),
                    task_short: "cccc1111".into(),
                    kind: gc_kind::CHAT.to_string(),
                    parent_id: "chat-1".into(),
                    ..Default::default()
                },
                TaskDiskUsage {
                    workspace_id: ws_a.to_string(),
                    task_short: "dddd1111".into(),
                    kind: DISK_USAGE_KIND_UNKNOWN.to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut asked: HashMap<String, Vec<String>> = HashMap::new();
        {
            let mut fetch = |workspace_id: &str, issue_ids: &[String]| {
                asked
                    .entry(workspace_id.to_string())
                    .or_default()
                    .extend(issue_ids.iter().cloned());
                Ok(issue_ids
                    .iter()
                    .filter_map(|id| match id.as_str() {
                        "issue-1" => Some((id.clone(), "done".to_string())),
                        "issue-2" => Some((id.clone(), "in_progress".to_string())),
                        "issue-3" => Some((id.clone(), "cancelled".to_string())),
                        _ => None,
                    })
                    .collect())
            };
            resolve_parent_statuses(&mut report, Some(&mut fetch)).unwrap();
        }

        let want = ["done", "done", "in_progress", "cancelled", "", ""];
        for (i, want_status) in want.iter().enumerate() {
            assert_eq!(
                report.tasks[i].parent_status, *want_status,
                "task[{i}] ({})",
                report.tasks[i].task_short
            );
        }
        assert_eq!(asked[ws_a].len(), 2, "de-duplicated ids");
        assert_eq!(asked[ws_b].len(), 1);
    }

    /// TestResolveParentStatuses_UnresolvedStaysBlank (diskusage_test.go:542–564).
    #[test]
    fn resolve_parent_statuses_unresolved_stays_blank() {
        let ws_id = "11111111-1111-1111-1111-111111111111";
        let mut report = DiskUsageReport {
            tasks: vec![
                issue_task(ws_id, "aaaa1111", "issue-known"),
                issue_task(ws_id, "aaaa2222", "issue-missing"),
            ],
            ..Default::default()
        };

        let mut fetch = |_: &str, ids: &[String]| {
            Ok(ids
                .iter()
                .filter(|id| id.as_str() == "issue-known")
                .map(|id| (id.clone(), "todo".to_string()))
                .collect())
        };
        resolve_parent_statuses(&mut report, Some(&mut fetch)).unwrap();
        assert_eq!(report.tasks[0].parent_status, "todo");
        assert_eq!(report.tasks[1].parent_status, "", "missing stays blank");
    }

    /// TestResolveParentStatuses_ChunksLargeWorkspaces (diskusage_test.go:569–605).
    #[test]
    fn resolve_parent_statuses_chunks_large_workspaces() {
        let ws_id = "11111111-1111-1111-1111-111111111111";
        let total = ISSUE_GC_BATCH_SIZE + 1;
        let tasks: Vec<TaskDiskUsage> = (0..total)
            .map(|i| issue_task(ws_id, &format!("task{i:04}"), &format!("issue-{i:04}")))
            .collect();
        let mut report = DiskUsageReport {
            tasks,
            ..Default::default()
        };

        let mut chunk_sizes: Vec<usize> = Vec::new();
        {
            let mut fetch = |_: &str, ids: &[String]| {
                chunk_sizes.push(ids.len());
                Ok(ids
                    .iter()
                    .map(|id| (id.clone(), "done".to_string()))
                    .collect())
            };
            resolve_parent_statuses(&mut report, Some(&mut fetch)).unwrap();
        }

        assert_eq!(chunk_sizes, vec![ISSUE_GC_BATCH_SIZE, 1]);
        assert!(report.tasks.iter().all(|t| t.parent_status == "done"));
    }

    /// TestResolveParentStatuses_PartialFailureKeepsOtherWorkspaces
    /// (diskusage_test.go:610–637).
    #[test]
    fn resolve_parent_statuses_partial_failure_keeps_other_workspaces() {
        let ws_good = "11111111-1111-1111-1111-111111111111";
        let ws_bad = "22222222-2222-2222-2222-222222222222";
        let mut report = DiskUsageReport {
            tasks: vec![
                issue_task(ws_good, "aaaa1111", "issue-good"),
                issue_task(ws_bad, "bbbb1111", "issue-bad"),
            ],
            ..Default::default()
        };

        let mut fetch = |workspace_id: &str, _: &[String]| {
            if workspace_id == ws_bad {
                anyhow::bail!("boom");
            }
            Ok([("issue-good".to_string(), "done".to_string())]
                .into_iter()
                .collect())
        };
        let err = resolve_parent_statuses(&mut report, Some(&mut fetch));
        assert!(err.is_err(), "failing workspace's error must surface");
        assert_eq!(report.tasks[0].parent_status, "done");
        assert_eq!(report.tasks[1].parent_status, "");
    }

    /// TestResolveParentStatuses_NoFetcherIsNoOp (diskusage_test.go:642–660).
    #[test]
    fn resolve_parent_statuses_no_fetcher_is_no_op() {
        let mut report = DiskUsageReport {
            tasks: vec![issue_task(
                "11111111-1111-1111-1111-111111111111",
                "aaaa1111",
                "issue-1",
            )],
            ..Default::default()
        };
        resolve_parent_statuses(&mut report, None).unwrap();
        assert_eq!(report.tasks[0].parent_status, "");
    }

    /// TestScanDiskUsage_ReportsRepoCacheSeparately (diskusage_test.go:666–694).
    #[test]
    fn reports_repo_cache_separately() {
        let root = tempfile::tempdir().unwrap();
        let ws_id = "11111111-1111-1111-1111-111111111111";
        write_file(
            &root
                .path()
                .join(ws_id)
                .join("aaaaaaaa")
                .join("workdir/main.go"),
            1000,
        );
        write_file(
            &root
                .path()
                .join(".repos")
                .join(ws_id)
                .join("widgets.git")
                .join("objects/pack/x"),
            4000,
        );
        write_file(
            &root
                .path()
                .join(".repos")
                .join(ws_id)
                .join("gadgets.git")
                .join("objects/pack/y"),
            2000,
        );

        let report = scan_disk_usage(root.path().to_str().unwrap(), &[]).unwrap();
        assert_eq!(report.repo_cache_size_bytes, 6000);
        assert_eq!(report.repo_cache_count, 2);
        assert_eq!(report.total_size_bytes, 1000, "cache excluded from totals");
        assert_eq!(report.total_workspace_count, 1, ".repos is not a workspace");
    }

    /// TestScanDiskUsage_SkipsDaemonInternalDotDirs (diskusage_test.go:699–723).
    #[test]
    fn skips_daemon_internal_dot_dirs() {
        let root = tempfile::tempdir().unwrap();
        let ws_id = "11111111-1111-1111-1111-111111111111";
        write_file(
            &root
                .path()
                .join(ws_id)
                .join("aaaaaaaa")
                .join("workdir/main.go"),
            1000,
        );
        write_file(
            &root
                .path()
                .join(".skill-cache")
                .join("v1")
                .join("bundle")
                .join("skill.md"),
            500,
        );

        let report = scan_disk_usage(root.path().to_str().unwrap(), &[]).unwrap();
        assert_eq!(report.total_workspace_count, 1);
        for ws in &report.workspaces {
            assert!(
                !ws.workspace_id.starts_with('.'),
                "dot-directory reported as workspace"
            );
        }
        assert_eq!(report.total_size_bytes, 1000);
    }
}
