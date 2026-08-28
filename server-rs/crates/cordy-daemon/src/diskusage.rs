//! Disk-usage scanning and reporting. Artifact matching is shared through
//! [`crate::artifact_matcher`] (CORD-12).
//!
//! Symbol map (Go → Rust):
//! - `TaskDiskUsage` / `WorkspaceDiskUsage` / `DiskUsageReport` /
//!   `DiskUsageRoot` / `RootDiskUsage` / `AggregateDiskUsageReport` →
//!   same-named structs
//! - `DiskUsageKindUnknown` → [`DISK_USAGE_KIND_UNKNOWN`]
//! - `ScanDiskUsageRoots` → [`scan_disk_usage_roots`]
//! - `ScanDiskUsage` → [`scan_disk_usage`]
//! - `repoCacheSize` / `ratio` / `buildTaskUsage` / `parentIDForMeta` /
//!   `taskSize` / `ShortID` → same-named fns
//! - `ParentStatusFetcher` → [`ParentStatusResolver`]
//! - `ResolveParentStatuses` → [`resolve_parent_statuses`]
//!
//! Port notes:
//! - `dirSize` and `ShortID` reuse gc.rs/repocache.rs equivalents (Go's
//!   diskusage.go re-implemented shortID because execenv.shortID is
//!   unexported; here both live in one crate).
//! - Go's `filepath.WalkDir` with SkipDir becomes a hand-rolled recursive walk
//!   using symlink_metadata so junction/symlink semantics match (never follow,
//!   never count).
//! - Map iteration order in Go randomizes workspace aggregation order before
//!   sort; here a BTreeMap keeps deterministic iteration, then the same sort
//!   applies. Output ordering contract is identical (sorted by size desc).

// S9-integration: consumed by the daemon CLI `disk-usage` command (S10 bins)
// and lane B wiring; silence dead-code until then.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::artifact_matcher::ArtifactMatcher;
use crate::client::Client;
use crate::execenv::execenv::{read_gc_meta, GCMetaKind, GcMeta};
use crate::gc::{dir_size, REPOS_DIR_NAME};
use crate::repocache::short_id;
use crate::repocache::{CancelCause, Ctx};

pub use crate::config::{artifact_patterns_from_env, resolve_workspaces_root};

/// `issueGCBatchSize` (gc.go:195): same chunk size the GC loop uses so one
/// oversized root cannot trip the server's batch cap.
pub(crate) const ISSUE_GC_BATCH_SIZE: usize = 500;

/// `DiskUsageKindUnknown`: kind for task dirs whose .gc_meta.json is missing
/// or unreadable — present on disk, but no parent record we can lock onto.
pub const DISK_USAGE_KIND_UNKNOWN: &str = "unknown";

/// `TaskDiskUsage`: one task workdir's footprint on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskDiskUsage {
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "workspace_short")]
    pub workspace_short: String,
    #[serde(rename = "task_short")]
    pub task_short: String,
    pub path: String,
    pub kind: String,
    #[serde(
        rename = "parent_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub parent_id: String,
    #[serde(rename = "parent_status")]
    pub parent_status: String,
    #[serde(rename = "age_seconds")]
    pub age_seconds: i64,
    #[serde(rename = "size_bytes")]
    pub size_bytes: i64,
    #[serde(rename = "artifact_size_bytes")]
    pub artifact_size_bytes: i64,
}

/// `WorkspaceDiskUsage`: per-workspace footprint across all tasks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceDiskUsage {
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

/// `DiskUsageReport`: full result of a single scan. Total* fields always
/// reflect the entire scan, never the post-`--top` truncated view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskUsageReport {
    #[serde(rename = "workspaces_root")]
    pub workspaces_root: String,
    #[serde(rename = "generated_at")]
    pub generated_at: chrono::DateTime<chrono::Utc>,
    #[serde(rename = "artifact_patterns")]
    pub artifact_patterns: Vec<String>,
    #[serde(rename = "managed_artifact_subpaths")]
    pub managed_artifact_subpaths: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<TaskDiskUsage>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceDiskUsage>,
    #[serde(rename = "total_task_count")]
    pub total_task_count: i64,
    #[serde(rename = "total_workspace_count")]
    pub total_workspace_count: i64,
    #[serde(rename = "total_size_bytes")]
    pub total_size_bytes: i64,
    #[serde(rename = "total_artifact_size_bytes")]
    pub total_artifact_size_bytes: i64,
    #[serde(rename = "total_artifact_ratio")]
    pub total_artifact_ratio: f64,
    /// Bare-repo cache (.repos) footprint — a sibling of task dirs, reported
    /// separately and excluded from totals.
    #[serde(rename = "repo_cache_size_bytes")]
    pub repo_cache_size_bytes: i64,
    #[serde(rename = "repo_cache_count")]
    pub repo_cache_count: i64,
}

/// `DiskUsageRoot`: a workspaces root plus the profile it was derived from
/// ("" = default root).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskUsageRoot {
    pub profile: String,
    pub root: String,
}

/// `RootDiskUsage`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RootDiskUsage {
    #[serde(default)]
    pub profile: String,
    pub report: DiskUsageReport,
}

/// `AggregateDiskUsageReport`: result of scanning several roots in one pass;
/// grand totals across every root's FULL scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregateDiskUsageReport {
    #[serde(rename = "generated_at")]
    pub generated_at: chrono::DateTime<chrono::Utc>,
    #[serde(rename = "artifact_patterns")]
    pub artifact_patterns: Vec<String>,
    #[serde(rename = "managed_artifact_subpaths")]
    pub managed_artifact_subpaths: Vec<String>,
    #[serde(default)]
    pub roots: Vec<RootDiskUsage>,
    #[serde(rename = "total_task_count")]
    pub total_task_count: i64,
    #[serde(rename = "total_workspace_count")]
    pub total_workspace_count: i64,
    #[serde(rename = "total_size_bytes")]
    pub total_size_bytes: i64,
    #[serde(rename = "total_artifact_size_bytes")]
    pub total_artifact_size_bytes: i64,
    #[serde(rename = "total_artifact_ratio")]
    pub total_artifact_ratio: f64,
    #[serde(rename = "total_repo_cache_size_bytes")]
    pub total_repo_cache_size_bytes: i64,
    #[serde(rename = "total_repo_cache_count")]
    pub total_repo_cache_count: i64,
}

/// `ratio` (diskusage.go:278): maps any zero denominator to 0 instead of NaN.
fn ratio(numerator: i64, denominator: i64) -> f64 {
    if denominator <= 0 {
        return 0.0;
    }
    numerator as f64 / denominator as f64
}

/// `repoCacheSize` (diskusage.go:250): measures the bare-repo cache and counts
/// second-level directories — the unit the GC evicts.
fn repo_cache_size(repos_root: &str) -> (i64, usize) {
    let Ok(ws_entries) = std::fs::read_dir(repos_root) else {
        return (0, 0);
    };
    let mut size_bytes: i64 = 0;
    let mut repo_count = 0usize;
    for ws_entry in ws_entries.flatten() {
        if !ws_entry.path().is_dir() {
            continue;
        }
        let Ok(repo_entries) = std::fs::read_dir(ws_entry.path()) else {
            continue;
        };
        for repo_entry in repo_entries.flatten() {
            if !repo_entry.path().is_dir() {
                continue;
            }
            repo_count += 1;
            size_bytes += dir_size(&repo_entry.path());
        }
    }
    (size_bytes, repo_count)
}

/// `ScanDiskUsageRoots` (diskusage.go:112): scans every root in order and
/// returns the combined report. A missing root yields an empty per-root report
/// (not an error); a genuinely unreadable root aborts the whole scan.
pub fn scan_disk_usage_roots(
    roots: &[DiskUsageRoot],
    artifact_patterns: &[String],
) -> anyhow::Result<AggregateDiskUsageReport> {
    let mut agg = AggregateDiskUsageReport {
        generated_at: chrono::Utc::now(),
        ..Default::default()
    };
    let matcher = ArtifactMatcher::new(
        artifact_patterns,
        &crate::execenv::reclaimable::managed_reclaimable_artifact_subpaths(),
    );
    agg.artifact_patterns = matcher.basenames_sorted();
    agg.managed_artifact_subpaths = matcher.managed_subpaths();

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

/// `ScanDiskUsage` (diskusage.go:151): walks workspacesRoot read-only, never
/// follows symlinks, counts only regular files. Artifact footprint matches
/// what the GC would actually reclaim (.git counts toward the total but not
/// artifacts). Missing roots return an empty report, not an error. Purely
/// local: parent_status stays empty until resolve_parent_statuses runs.
pub fn scan_disk_usage(
    workspaces_root: &str,
    artifact_patterns: &[String],
) -> anyhow::Result<DiskUsageReport> {
    let mut report = DiskUsageReport {
        workspaces_root: workspaces_root.to_string(),
        generated_at: chrono::Utc::now(),
        ..Default::default()
    };
    if workspaces_root.is_empty() {
        anyhow::bail!("disk-usage: workspaces root is required");
    }

    let matcher = ArtifactMatcher::new(
        artifact_patterns,
        &crate::execenv::reclaimable::managed_reclaimable_artifact_subpaths(),
    );
    report.artifact_patterns = matcher.basenames_sorted();
    report.managed_artifact_subpaths = matcher.managed_subpaths();

    let Ok(ws_entries) = std::fs::read_dir(workspaces_root) else {
        let err = std::fs::metadata(workspaces_root).err();
        if err.is_some() && err.unwrap().kind() != std::io::ErrorKind::NotFound {
            // Distinguish not-found from genuinely-unreadable like os.IsNotExist.
        }
        if !Path::new(workspaces_root).exists() {
            return Ok(report);
        }
        anyhow::bail!("disk-usage: read workspaces root");
    };

    let mut ws_agg: BTreeMap<String, WorkspaceDiskUsage> = BTreeMap::new();

    for ws_entry in ws_entries.flatten() {
        let ws_is_dir = ws_entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !ws_is_dir {
            continue;
        }
        let name = ws_entry.file_name().to_string_lossy().into_owned();
        // The bare-repo cache is not a workspace: measured separately because
        // it is reclaimed on its own schedule (GCRepoTTL).
        if name == REPOS_DIR_NAME {
            let (size, count) =
                repo_cache_size(&Path::new(workspaces_root).join(&name).to_string_lossy());
            report.repo_cache_size_bytes = size;
            report.repo_cache_count = count as i64;
            continue;
        }
        // Dot-directories are daemon-internal caches, never workspaces.
        if name.starts_with('.') {
            continue;
        }
        let ws_id = name;
        let ws_dir = Path::new(workspaces_root).join(&ws_id);
        let Ok(task_entries) = std::fs::read_dir(&ws_dir) else {
            continue;
        };
        for t in task_entries.flatten() {
            if !t.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }
            let task_name = t.file_name().to_string_lossy().into_owned();
            let task_dir = ws_dir.join(&task_name);
            let usage = build_task_usage(&task_dir.to_string_lossy(), &ws_id, &task_name, &matcher);

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

    report
        .tasks
        .sort_by_key(|t| std::cmp::Reverse(t.size_bytes));

    let mut workspaces: Vec<WorkspaceDiskUsage> = ws_agg.into_values().collect();
    for ws in &mut workspaces {
        ws.artifact_ratio = ratio(ws.artifact_size_bytes, ws.size_bytes);
    }
    workspaces.sort_by_key(|w| std::cmp::Reverse(w.size_bytes));
    report.workspaces = workspaces;

    report.total_task_count = report.tasks.len() as i64;
    report.total_workspace_count = report.workspaces.len() as i64;
    report.total_artifact_ratio = ratio(report.total_artifact_size_bytes, report.total_size_bytes);

    Ok(report)
}

/// `buildTaskUsage` (diskusage.go:306).
fn build_task_usage(
    task_dir: &str,
    ws_id: &str,
    task_short: &str,
    matcher: &ArtifactMatcher,
) -> TaskDiskUsage {
    let mut usage = TaskDiskUsage {
        workspace_id: ws_id.to_string(),
        workspace_short: short_id(ws_id),
        task_short: task_short.to_string(),
        path: task_dir.to_string(),
        kind: DISK_USAGE_KIND_UNKNOWN.to_string(),
        ..Default::default()
    };

    let mut meta_present = false;
    if let Ok(meta) = read_gc_meta(task_dir) {
        meta_present = true;
        usage.kind = gc_kind_str(meta.kind.as_ref()).to_string();
        usage.parent_id = parent_id_for_meta(&meta);
        if let Some(completed_at) = meta.completed_at {
            usage.age_seconds = (chrono::Utc::now() - completed_at).num_seconds();
        } else if let Some(age) = gc_meta_file_age(task_dir) {
            usage.age_seconds = age.num_seconds();
        }
    }
    // No readable metadata: use taskDir mtime just like orphanByMTime.
    // Legacy readable metadata without completed_at uses its own file mtime.
    if usage.age_seconds <= 0 && !meta_present {
        if let Ok(info) = std::fs::metadata(task_dir) {
            if let Ok(modified) = info.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    usage.age_seconds = elapsed.as_secs() as i64;
                }
            }
        }
    }

    let (size_bytes, artifact_size_bytes) = task_size(Path::new(task_dir), matcher);
    usage.size_bytes = size_bytes;
    usage.artifact_size_bytes = artifact_size_bytes;
    usage
}

/// GCMetaKind → its persisted string form (Go: `string(meta.Kind)`).
fn gc_kind_str(kind: Option<&GCMetaKind>) -> &str {
    match kind {
        Some(GCMetaKind::Issue) => "issue",
        Some(GCMetaKind::Chat) => "chat",
        Some(GCMetaKind::AutopilotRun) => "autopilot_run",
        Some(GCMetaKind::QuickCreate) => "quick_create",
        Some(GCMetaKind::Other(kind)) => kind,
        None => "",
    }
}

/// The `.gc_meta.json` file mtime age (Go's gcMetaFileAge).
fn gc_meta_file_age(task_dir: &str) -> Option<chrono::Duration> {
    let info = std::fs::metadata(Path::new(task_dir).join(".gc_meta.json")).ok()?;
    let modified = info.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|_| {
            let m = chrono::DateTime::<chrono::Utc>::from(modified);
            chrono::Utc::now() - m
        })
}

/// `parentIDForMeta` (diskusage.go:342): only the field matching Kind is
/// meaningful.
fn parent_id_for_meta(meta: &GcMeta) -> String {
    match meta.kind.as_ref() {
        Some(GCMetaKind::Issue) => meta.issue_id.trim().to_string(),
        Some(GCMetaKind::Chat) => meta.chat_session_id.trim().to_string(),
        Some(GCMetaKind::AutopilotRun) => meta.autopilot_run_id.trim().to_string(),
        Some(GCMetaKind::QuickCreate) => meta.task_id.trim().to_string(),
        Some(GCMetaKind::Other(_)) | None => String::new(),
    }
}

/// `taskSize` (diskusage.go:449): returns (totalBytes, artifactBytes). Never
/// follows symlinks/junctions; counts only regular files. A matched artifact
/// directory counts into BOTH totals without descending. A .git subtree counts
/// into totalBytes only — real footprint the GC would also remove on a clean.
fn task_size(task_dir: &Path, matcher: &ArtifactMatcher) -> (i64, i64) {
    if task_dir.as_os_str().is_empty() {
        return (0, 0);
    }
    let abs_root = match absolute(task_dir) {
        Ok(p) => p,
        Err(_) => return (0, 0),
    };
    let mut state = TaskSizeState {
        total_bytes: 0,
        artifact_bytes: 0,
    };
    walk_task_size(&abs_root, &abs_root, matcher, &mut state);
    (state.total_bytes, state.artifact_bytes)
}

struct TaskSizeState {
    total_bytes: i64,
    artifact_bytes: i64,
}

fn walk_task_size(
    abs_root: &Path,
    dir: &Path,
    matcher: &ArtifactMatcher,
    state: &mut TaskSizeState,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Links: never followed, never counted (symlink or junction).
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                state.total_bytes += dir_size(&path);
                continue;
            }
            let matched = path
                .file_name()
                .map(|n| n.to_string_lossy())
                .and_then(|name| matcher.match_directory(abs_root, &path, name.as_ref()));
            if let Some(_label) = matched {
                let size = dir_size(&path);
                state.total_bytes += size;
                state.artifact_bytes += size;
                continue;
            }
            walk_task_size(abs_root, &path, matcher, state);
            continue;
        }
        if meta.is_file() {
            state.total_bytes += meta.len() as i64;
        }
    }
}

/// filepath.Abs equivalent.
fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(path))
}

/// Resolves issue statuses without leaking the daemon's internal repository
/// context into a CLI or embedding caller.
#[async_trait]
pub trait ParentStatusResolver: Send + Sync {
    async fn fetch_parent_statuses(
        &self,
        cancellation: &CancellationToken,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, String>>;
}

/// Adapter over the daemon HTTP client. The client keeps its existing batch →
/// legacy fallback and request/error taxonomy; this boundary only translates
/// the public cancellation token into the internal request context.
pub struct ClientParentStatusResolver {
    client: Arc<Client>,
}

impl ClientParentStatusResolver {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ParentStatusResolver for ClientParentStatusResolver {
    async fn fetch_parent_statuses(
        &self,
        cancellation: &CancellationToken,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, String>> {
        let ctx = Ctx::new();
        if cancellation.is_cancelled() {
            ctx.cancel_with(CancelCause::Cancelled);
        }
        let result = tokio::select! {
            result = self.client.get_issue_gc_checks(&ctx, workspace_id, issue_ids) => result,
            () = cancellation.cancelled() => {
                ctx.cancel_with(CancelCause::Cancelled);
                Err(anyhow::anyhow!("context canceled"))
            }
        };
        result.map(|results| {
            results
                .into_iter()
                .filter_map(|(issue_id, result)| {
                    (result.err.is_none() && result.found).then_some((issue_id, result.status))
                })
                .collect()
        })
    }
}

/// `ResolveParentStatuses` (diskusage.go:376): fills ParentStatus on every
/// issue-kind task via batch fetches. Best-effort: a failed workspace leaves
/// its tasks unresolved and its error returned while other workspaces still
/// fill in.
pub async fn resolve_parent_statuses<R>(
    cancellation: &CancellationToken,
    report: &mut DiskUsageReport,
    resolver: &R,
) -> anyhow::Result<()>
where
    R: ParentStatusResolver + ?Sized,
{
    let mut ids_by_ws: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for task in &report.tasks {
        if task.kind != "issue" || task.parent_id.is_empty() {
            continue;
        }
        // Several task dirs can share one issue; de-duplicate server calls.
        if !seen
            .entry(task.workspace_id.clone())
            .or_default()
            .insert(task.parent_id.clone())
        {
            continue;
        }
        ids_by_ws
            .entry(task.workspace_id.clone())
            .or_default()
            .push(task.parent_id.clone());
    }
    if ids_by_ws.is_empty() {
        return Ok(());
    }

    let mut first_err: Option<anyhow::Error> = None;
    let mut statuses: BTreeMap<String, std::collections::HashMap<String, String>> = BTreeMap::new();
    for (workspace_id, ids) in ids_by_ws {
        let mut resolved = std::collections::HashMap::with_capacity(ids.len());
        for chunk in ids.chunks(ISSUE_GC_BATCH_SIZE) {
            match resolver
                .fetch_parent_statuses(cancellation, &workspace_id, chunk)
                .await
            {
                Err(err) => {
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
                Ok(chunk_result) => resolved.extend(chunk_result),
            }
        }
        statuses.insert(workspace_id, resolved);
    }

    for task in &mut report.tasks {
        if task.kind != "issue" || task.parent_id.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeParentStatusResolver;

    #[async_trait]
    impl ParentStatusResolver for FakeParentStatusResolver {
        async fn fetch_parent_statuses(
            &self,
            cancellation: &CancellationToken,
            workspace_id: &str,
            issue_ids: &[String],
        ) -> anyhow::Result<HashMap<String, String>> {
            if cancellation.is_cancelled() || workspace_id == "workspace-error" {
                anyhow::bail!("parent status lookup unavailable")
            }
            Ok(issue_ids
                .iter()
                .map(|id| (id.clone(), "in_progress".to_string()))
                .collect())
        }
    }

    #[test]
    fn report_dto_serializes_go_field_names_without_runtime_paths() {
        let report = DiskUsageReport {
            workspaces_root: "/tmp/workspaces".to_string(),
            total_task_count: 1,
            total_size_bytes: 12,
            ..Default::default()
        };
        let json = serde_json::to_value(report).expect("disk usage JSON");
        assert_eq!(json["workspaces_root"], "/tmp/workspaces");
        assert_eq!(json["total_task_count"], 1);
        assert!(json.get("totalTaskCount").is_none());
    }

    #[test]
    fn root_and_scan_boundaries_are_typed_and_fail_closed() {
        let root = DiskUsageRoot {
            profile: "staging".to_string(),
            root: "/tmp/staging-workspaces".to_string(),
        };
        let encoded = serde_json::to_value(&root).expect("root JSON");
        assert_eq!(encoded["profile"], "staging");
        assert_eq!(encoded["root"], "/tmp/staging-workspaces");
        assert!(scan_disk_usage("", &[]).is_err());
        let missing = scan_disk_usage("/path/that/does/not/exist", &["node_modules".into()])
            .expect("missing roots are empty reports");
        assert_eq!(missing.total_task_count, 0);
        assert_eq!(missing.artifact_patterns, vec!["node_modules"]);
        let resolved = resolve_workspaces_root("staging", "relative-root")
            .expect("relative override is made absolute");
        assert!(Path::new(&resolved).is_absolute());
    }

    #[tokio::test]
    async fn parent_status_resolution_deduplicates_and_keeps_best_effort_results() {
        let mut report = DiskUsageReport {
            tasks: vec![
                TaskDiskUsage {
                    workspace_id: "workspace-ok".into(),
                    kind: "issue".into(),
                    parent_id: "issue-1".into(),
                    ..Default::default()
                },
                TaskDiskUsage {
                    workspace_id: "workspace-ok".into(),
                    kind: "issue".into(),
                    parent_id: "issue-1".into(),
                    ..Default::default()
                },
                TaskDiskUsage {
                    workspace_id: "workspace-error".into(),
                    kind: "issue".into(),
                    parent_id: "issue-2".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let error = resolve_parent_statuses(
            &CancellationToken::new(),
            &mut report,
            &FakeParentStatusResolver,
        )
        .await
        .expect_err("one workspace failure is reported");
        assert!(error.to_string().contains("unavailable"));
        assert_eq!(report.tasks[0].parent_status, "in_progress");
        assert_eq!(report.tasks[1].parent_status, "in_progress");
        assert!(report.tasks[2].parent_status.is_empty());
    }

    #[tokio::test]
    async fn parent_status_resolution_honors_cancellation() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut report = DiskUsageReport {
            tasks: vec![TaskDiskUsage {
                workspace_id: "workspace-ok".into(),
                kind: "issue".into(),
                parent_id: "issue-1".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let error = resolve_parent_statuses(&cancellation, &mut report, &FakeParentStatusResolver)
            .await
            .expect_err("cancelled lookup is surfaced");
        assert!(error.to_string().contains("unavailable"));
        assert!(report.tasks[0].parent_status.is_empty());
    }
}
