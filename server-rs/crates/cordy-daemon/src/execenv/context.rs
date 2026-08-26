//! Port of execenv/context.go.
//!
//! Symbol map:
//! - TaskContextMarkerRelPath / TaskContextMarkerManagedBy
//!   → TASK_CONTEXT_MARKER_REL_PATH / TASK_CONTEXT_MARKER_MANAGED_BY
//! - taskContextMarkerFile        → TaskContextMarkerFile
//! - EnsureWorkspacesRootMarker   → ensure_workspaces_root_marker
//! - writeWorkspacesRootMarkerAtomic → write_workspaces_root_marker_atomic
//! - writeContextFiles            → write_context_files
//! - writeTaskContextMarker       → write_task_context_marker
//! - projectResourceFile          → ProjectResourceFile
//! - writeProjectResources        → write_project_resources
//! - resolveSkillsDir             → resolve_skills_dir
//! - skillsDirPath                → skills_dir_path
//! - ensureSkillFrontmatter       → ensure_skill_frontmatter
//! - frontmatterHasNameKey        → frontmatter_has_name_key
//! - synthesizeFrontmatter        → synthesize_frontmatter
//! - isFrontmatterValidYAML       → is_frontmatter_valid_yaml
//! - frontmatterParts             → frontmatter_parts
//! - frontmatterBodyStart         → frontmatter_body_start
//! - hasFrontmatterName           → has_frontmatter_name
//! - frontmatterNameValueSpan     → frontmatter_name_value_span
//! - setFrontmatterName           → set_frontmatter_name
//! - renameFrontmatterNameViaNode → rename_frontmatter_name_via_node
//! - materializeAliasesOf / cloneYAMLNode → folded into serde_yaml's alias
//!   resolution (aliases materialize at parse; anchors drop on re-marshal,
//!   which is exactly the outcome Go's manual pass produces)
//! - frontmatterNameIs            → frontmatter_name_is
//! - yamlEscapeInline             → yaml_escape_inline
//! - sanitizeSkillName            → sanitize_skill_name
//! - writeSkillFiles              → write_skill_files
//! - renderIssueContext           → render_issue_context
//! - renderQuickCreateContext     → render_quick_create_context
//! - renderAutopilotContext       → render_autopilot_context
//!
//! Shared package helpers hosted here per mod.rs (sidecar-manifest /
//! runtime-skill-policy land with lane E2/E3 consumers):
//! - sidecar_manifest.go: SidecarManifest, recordMkdirAll, recordWriteFile,
//!   errPathPreExists, allocateCollisionFreeSkillDir, skillSlugCandidate,
//!   writeSidecarManifest, CleanupSidecars, rollBackManifest,
//!   rollBackPreparedSidecars, removeReusedManagedSkillDirs, dirHasEntries
//! - runtime_skill_policy.go: RuntimeSkillRefForEnv, cleanRuntimeSkillKey,
//!   prepareClaudeSkillSettings, ensureCodexDisabledSkillsConfig,
//!   workspaceClaimsRuntimeSkill
//! - skill_visibility.go: resolveSkillSlugs (modelVisibleSkills stays with
//!   its runtime_config_sections consumer in a later lane)
//! - internal/skill.IsReservedContentPath → is_reserved_content_path
//! - pkg/agent.BuiltinRuntimeByID (SkillsDir only) → builtin_runtime_skills_dir
//!
//! Deviations:
//! - slog logger parameters dropped; tracing macros used directly.
//! - frontmatterNameValueSpan is lexical (line scan) instead of yaml.v3 node
//!   line numbers; setFrontmatterName re-parses and verifies the result and
//!   falls back to renameFrontmatterNameViaNode on any mismatch, so a
//!   mis-scanned span degrades to the same recovery path Go uses.
//! - renameFrontmatterNameViaNode re-marshals through serde_yaml, which
//!   normalizes quoting/indentation just like Go's yaml.Marshal round-trip.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};

pub(crate) use super::execenv::SkillContextForEnv;
use super::execenv::{clean_path, join_path, ProjectResourceForEnv, TaskContextForEnv};

// ---------------------------------------------------------------------------
// Daemon task marker
// ---------------------------------------------------------------------------

/// A non-secret marker the daemon writes under the task workdir. The CLI uses
/// it as a fallback daemon-task signal when a child sandbox strips all
/// CORDY_* env vars before invoking `cordy`.
pub const TASK_CONTEXT_MARKER_REL_PATH: &str = ".cordy/daemon_task_context.json";

/// The marker discriminator the CLI checks before treating
/// TASK_CONTEXT_MARKER_REL_PATH as daemon-owned.
pub const TASK_CONTEXT_MARKER_MANAGED_BY: &str = "cordy-daemon-task";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskContextMarkerFile {
    #[serde(rename = "managed_by")]
    pub managed_by: String,
    #[serde(rename = "agent_id", skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    #[serde(rename = "issue_id", skip_serializing_if = "String::is_empty")]
    pub issue_id: String,
    #[serde(rename = "chat_session_id", skip_serializing_if = "String::is_empty")]
    pub chat_session_id: String,
}

/// EnsureWorkspacesRootMarker writes a persistent daemon-task marker at
/// {workspaces_root}/.cordy/daemon_task_context.json.
///
/// The per-workdir marker only protects `cordy` invocations whose cwd is
/// inside the workdir, because the CLI discovers markers by walking *up* from
/// cwd. Every directory under workspacesRoot is daemon-owned, so a marker at
/// the root puts the entire tree back under the fail-closed guard without
/// touching any directory a user works in.
///
/// A pre-existing marker owned by the daemon is left untouched. A truncated or
/// otherwise unparseable file — the signature of a torn write from a daemon
/// killed mid-write — is reclaimed and rewritten. Only a *parseable* marker
/// owned by something else is treated as genuinely foreign and refused. The
/// (re)write is atomic (temp file + rename).
pub fn ensure_workspaces_root_marker(workspaces_root: &str) -> anyhow::Result<()> {
    if workspaces_root.trim().is_empty() {
        anyhow::bail!("execenv: workspaces root is required");
    }
    let path = join_path(&[workspaces_root, TASK_CONTEXT_MARKER_REL_PATH]);
    match std::fs::read_to_string(&path) {
        Ok(existing) => {
            if let Ok(marker) = serde_json::from_str::<TaskContextMarkerFile>(&existing) {
                if marker.managed_by == TASK_CONTEXT_MARKER_MANAGED_BY {
                    return Ok(());
                }
                // Parseable but owned by something else: never clobber it.
                anyhow::bail!(
                    "foreign file at workspaces root marker path {path}; refusing to overwrite"
                );
            }
            // Unparseable content is almost certainly a torn write of our own
            // marker; fall through to reclaim it below.
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            // A real read error (e.g. a directory at the path) is not a signal
            // we can safely overwrite; surface it. Callers degrade non-fatally.
            return Err(
                anyhow::Error::new(e).context(format!("read workspaces root marker {path}"))
            );
        }
        Err(_) => {}
    }
    let payload = TaskContextMarkerFile {
        managed_by: TASK_CONTEXT_MARKER_MANAGED_BY.to_string(),
        ..Default::default()
    };
    let data = serde_json::to_vec_pretty(&payload).context("marshal workspaces root marker")?;
    let dir = dir_of(&path);
    std::fs::create_dir_all(&dir).context("create workspaces root marker dir")?;
    write_workspaces_root_marker_atomic(&path, &data).context("write workspaces root marker")?;
    Ok(())
}

/// Writes data to path via a same-directory temp file plus a rename, so a
/// concurrent reader observes either the old file or the complete new one —
/// never a partial write. Perm is 0644 because the CLI's upward walk must be
/// able to read the marker from a subprocess that may run under a different
/// uid; the payload is non-secret.
fn write_workspaces_root_marker_atomic(path: &str, data: &[u8]) -> anyhow::Result<()> {
    let dir = Path::new(path).parent().unwrap_or(Path::new("."));
    let tmp = tempfile::Builder::new()
        .prefix(".daemon_task_context-")
        .suffix(".json.tmp")
        .tempfile_in(dir)
        .context("create temp workspaces root marker")?;
    let tmp_path = tmp.path().to_path_buf();
    {
        use std::io::Write as _;
        let mut f = tmp.as_file();
        f.write_all(data).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            anyhow::Error::new(e).context("write temp workspaces root marker")
        })?;
    }
    set_file_mode_0644(&tmp_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::Error::new(e).context("chmod temp workspaces root marker")
    })?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::Error::new(e).context("rename workspaces root marker")
    })?;
    Ok(())
}

fn set_file_mode_0644(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sidecar manifest (hosted from sidecar_manifest.go — see module header)
// ---------------------------------------------------------------------------

/// On-disk JSON Prepare writes into envRoot to record every file and
/// intermediate directory it created inside WorkDir. CleanupSidecars reads it
/// back to roll the workdir to its pre-Prepare state. The file lives in
/// envRoot (daemon scratch), never in WorkDir.
const SIDECAR_MANIFEST_FILE: &str = ".cordy_sidecar_manifest.json";

/// Sentinel record_write_file returns when the target path already exists.
/// The manifest contract is that we never mutate paths we don't own: a
/// pre-existing file belongs to the user and the write must be refused so
/// cleanup can be a pure deletion of paths we created.
#[derive(Debug, thiserror::Error)]
#[error("execenv: refuse to overwrite pre-existing path")]
pub(crate) struct ErrPathPreExists;

fn err_pre_exists(path: &str) -> anyhow::Error {
    anyhow::Error::new(ErrPathPreExists).context(format!("path exists: {path}"))
}

pub(crate) fn is_pre_exists(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ErrPathPreExists>().is_some()
}

/// Records the filesystem mutations writeContextFiles and its callees make
/// inside the agent's WorkDir for a single task. Files lists absolute paths of
/// regular files we created; Dirs lists absolute paths of directories we
/// created in root-first creation order (cleanup walks in reverse).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarManifest {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub dirs: Vec<String>,
}

/// Behaves like create_dir_all but additionally records every parent directory
/// it had to create (skipping any that already existed) into m so cleanup can
/// rmdir them later. When m is None this is identical to create_dir_all.
pub(crate) fn record_mkdir_all(path: &str, m: Option<&mut SidecarManifest>) -> anyhow::Result<()> {
    let Some(m) = m else {
        return std::fs::create_dir_all(path).map_err(anyhow::Error::new);
    };
    // Walk leaf-first, collecting ancestors that don't currently exist. We
    // stop at the first existing ancestor (or the filesystem root) so
    // pre-existing user directories are never recorded.
    let mut to_create: Vec<String> = Vec::new();
    let mut cur = clean_path(path);
    loop {
        match std::fs::symlink_metadata(&cur) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("stat ancestor {cur}")));
            }
        }
        to_create.push(cur.clone());
        let parent = Path::new(&cur).parent().map(|p| p.to_path_buf());
        match parent {
            Some(p) if !p.as_os_str().is_empty() && p != Path::new(&cur) && p != Path::new(".") => {
                cur = p.to_string_lossy().into_owned();
            }
            _ => break,
        }
    }
    std::fs::create_dir_all(path)?;
    // Reverse leaf-first → root-first so cleanup can reverse-iterate to peel
    // directories from the leaves upward.
    to_create.reverse();
    m.dirs.extend(to_create);
    Ok(())
}

/// Writes data to path with 0644 and records the path in m for later cleanup,
/// but ONLY when path does not already exist. Any existing entry — regular
/// file, symlink, directory — is a collision and refuses to be touched.
/// When m is None this collapses to a plain write.
pub(crate) fn record_write_file(
    path: &str,
    data: &[u8],
    m: Option<&mut SidecarManifest>,
) -> anyhow::Result<()> {
    let Some(m) = m else {
        std::fs::write(path, data)?;
        return Ok(());
    };
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Err(err_pre_exists(path)),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(anyhow::Error::new(e).context(format!("stat target {path}")));
        }
        Err(_) => {}
    }
    std::fs::write(path, data)?;
    m.files.push(path.to_string());
    Ok(())
}

/// Picks a directory under skills_parent whose path does NOT currently exist,
/// so write_skill_files can lay down a Cordy skill without colliding with a
/// user-installed skill of the same slug. First attempt is always the natural
/// baseSlug; on collision we append `-cordy`, then `-cordy-2`, `-cordy-3`, …
/// bounded to 64 attempts.
pub(crate) fn allocate_collision_free_skill_dir(
    skills_parent: &str,
    base_slug: &str,
) -> anyhow::Result<(String, String)> {
    const MAX_ATTEMPTS: usize = 64;
    for i in 0..MAX_ATTEMPTS {
        let candidate = skill_slug_candidate(base_slug, i);
        let path = join_path(&[skills_parent, &candidate]);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((candidate, path));
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("stat candidate {path}")));
            }
            Ok(_) => {}
        }
    }
    anyhow::bail!(
        "allocate collision-free skill dir under {skills_parent}: exhausted {MAX_ATTEMPTS} attempts for base {base_slug:?}"
    )
}

/// The nth name to try for a skill whose natural slug is baseSlug: the bare
/// slug first, then `-cordy`, then numbered variants. Two callers must agree
/// on this sequence — allocate_collision_free_skill_dir (filesystem probe) and
/// resolve_skill_slugs (in-memory dedup).
pub(crate) fn skill_slug_candidate(base_slug: &str, attempt: usize) -> String {
    match attempt {
        0 => base_slug.to_string(),
        1 => format!("{base_slug}-cordy"),
        n => format!("{base_slug}-cordy-{n}"),
    }
}

/// Persists m to {env_root}/{SIDECAR_MANIFEST_FILE}. Empty manifests are still
/// written so a later cleanup that finds the file knows tracking was attempted.
pub(crate) fn write_sidecar_manifest(env_root: &str, m: &SidecarManifest) -> anyhow::Result<()> {
    if env_root.is_empty() {
        return Ok(());
    }
    let data = serde_json::to_vec(m).context("marshal sidecar manifest")?;
    std::fs::write(Path::new(env_root).join(SIDECAR_MANIFEST_FILE), data)?;
    Ok(())
}

/// Rolls the user's workdir back to its pre-Prepare state by removing every
/// file the manifest at env_root records and then rmdir-ing every directory it
/// records, deepest first. ENOENT and non-empty directories are tolerated;
/// real I/O errors are surfaced after continuing the remaining entries.
pub(crate) fn cleanup_sidecars(env_root: &str) -> anyhow::Result<()> {
    if env_root.is_empty() {
        return Ok(());
    }
    let manifest_path = Path::new(env_root).join(SIDECAR_MANIFEST_FILE);
    let data = match std::fs::read(&manifest_path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("read sidecar manifest {}", manifest_path.display())));
        }
    };
    let m: SidecarManifest = serde_json::from_slice(&data).map_err(|e| {
        anyhow::Error::new(e).context(format!(
            "parse sidecar manifest {}",
            manifest_path.display()
        ))
    })?;

    roll_back_manifest(&m, Some(manifest_path.to_string_lossy().as_ref()))
}

/// Removes everything m records, then removes manifest_path itself when given.
/// Shared body of cleanup_sidecars (reads m back from disk after the task ran)
/// and roll_back_prepared_sidecars (in-memory manifest of a Prepare that never
/// finished, no manifest file to delete).
fn roll_back_manifest(m: &SidecarManifest, manifest_path: Option<&str>) -> anyhow::Result<()> {
    let mut first_err: Option<anyhow::Error> = None;
    let mut capture = |err: anyhow::Error| {
        if first_err.is_none() {
            first_err = Some(err);
        }
    };

    for f in &m.files {
        if let Err(e) = std::fs::remove_file(f) {
            if e.kind() != std::io::ErrorKind::NotFound {
                capture(anyhow::Error::new(e).context(format!("remove {f}")));
            }
        }
    }

    // Reverse iterate so the deepest directory is tried first. When rmdir
    // fails we re-read the directory to tell ENOTEMPTY (user content present —
    // skip silently) apart from real I/O errors (capture and surface).
    for d in m.dirs.iter().rev() {
        match std::fs::remove_dir(d) {
            Ok(()) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                let (has_entries, ok) = dir_has_entries(d);
                match ok {
                    false => capture(anyhow::Error::new(e).context(format!("rmdir {d}"))),
                    true if has_entries => {}
                    true => capture(anyhow::Error::new(e).context(format!("rmdir {d}"))),
                }
            }
        }
    }

    if let Some(mp) = manifest_path {
        if let Err(e) = std::fs::remove_file(mp) {
            if e.kind() != std::io::ErrorKind::NotFound {
                capture(anyhow::Error::new(e).context(format!("remove manifest {mp}")));
            }
        }
    }

    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Undoes the sidecar writes of a Prepare that failed before it could persist
/// the manifest, using the in-memory manifest Prepare was still filling in
/// (MUL-6132). There is no manifest file to remove.
pub(crate) fn roll_back_prepared_sidecars(m: &SidecarManifest) -> anyhow::Result<()> {
    roll_back_manifest(m, None)
}

/// Force-removes the skill directories the prior dispatch recorded under
/// skills_parent in its sidecar manifest at env_root, even when they are now
/// non-empty (#3684 reuse-path companion to cleanup_sidecars). Only
/// directories whose immediate parent is skills_parent are removed, so the
/// blast radius is exactly the platform's own skill roots.
pub(crate) fn remove_reused_managed_skill_dirs(
    env_root: &str,
    skills_parent: &str,
) -> anyhow::Result<()> {
    if env_root.is_empty() || skills_parent.is_empty() {
        return Ok(());
    }
    let data = match std::fs::read(Path::new(env_root).join(SIDECAR_MANIFEST_FILE)) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(
                anyhow::Error::new(e).context("read sidecar manifest for reuse skill rollback")
            );
        }
    };
    let m: SidecarManifest = serde_json::from_slice(&data).map_err(|e| {
        anyhow::Error::new(e).context("parse sidecar manifest for reuse skill rollback")
    })?;

    let clean_parent = clean_path(skills_parent);
    let mut first_err: Option<anyhow::Error> = None;
    for d in &m.dirs {
        if dir_of(d) != clean_parent {
            continue;
        }
        if let Err(e) = super::execenv::remove_tree(d) {
            if first_err.is_none() {
                first_err = Some(e.context(format!("remove managed skill dir {d}")));
            }
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Inspects dir and reports whether it currently contains any entries. The
/// second value distinguishes three states: (false, true) — empty or gone;
/// (true, true) — user content present; (_, false) — readdir itself failed.
fn dir_has_entries(dir: &str) -> (bool, bool) {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => match entries.next() {
            Some(Ok(_)) => (true, true),
            Some(Err(_)) | None => (false, true),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, true),
        Err(_) => (false, false),
    }
}

// ---------------------------------------------------------------------------
// Context files
// ---------------------------------------------------------------------------

/// Renders and writes .agent_context/issue_context.md and skills into the
/// appropriate provider-native location. See context.go for the full
/// provider table. manifest, when Some, is populated with every file we
/// created and every intermediate directory we had to create.
pub(crate) fn write_context_files(
    work_dir: &str,
    provider: &str,
    ctx: &TaskContextForEnv,
    mut manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<()> {
    write_task_context_marker(work_dir, ctx, manifest.as_deref_mut())?;

    let context_dir = join_path(&[work_dir, ".agent_context"]);
    record_mkdir_all(&context_dir, manifest.as_deref_mut()).context("create .agent_context dir")?;

    let content = render_issue_context(ctx);
    let path = join_path(&[&context_dir, "issue_context.md"]);
    if let Err(err) = record_write_file(&path, content.as_bytes(), manifest.as_deref_mut()) {
        // A pre-existing path means the user already owns
        // .agent_context/issue_context.md — refusing the write is correct:
        // the runtime brief already carries every fact this file would.
        if !is_pre_exists(&err) {
            return Err(err.context("write issue_context.md"));
        }
    }

    if !ctx.agent_skills.is_empty() && provider != "hermes" {
        let skills_dir = resolve_skills_dir(work_dir, provider, manifest.as_deref_mut())
            .context("resolve skills dir")?;
        // Codex skills are written to codex-home in prepare; skip here.
        if provider != "codex" {
            write_skill_files(&skills_dir, &ctx.agent_skills, manifest.as_deref_mut())
                .context("write skill files")?;
        }
    }

    // Project resources are best-effort: a write failure logs but does not
    // block task startup.
    write_project_resources(work_dir, ctx, manifest).context("write project resources")?;

    Ok(())
}

fn write_task_context_marker(
    work_dir: &str,
    ctx: &TaskContextForEnv,
    mut manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<()> {
    let marker_path = join_path(&[work_dir, TASK_CONTEXT_MARKER_REL_PATH]);
    let dir = dir_of(&marker_path);
    record_mkdir_all(&dir, manifest.as_deref_mut()).context("create .cordy dir")?;
    // The sidecar manifest removes this marker on normal local_directory
    // cleanup. If a crash leaves it behind, the CLI intentionally treats it
    // as daemon context and fails closed instead of using a user PAT.
    let payload = TaskContextMarkerFile {
        managed_by: TASK_CONTEXT_MARKER_MANAGED_BY.to_string(),
        agent_id: ctx.agent_id.clone(),
        issue_id: ctx.issue_id.clone(),
        chat_session_id: ctx.chat_session_id.clone(),
    };
    let data = serde_json::to_vec_pretty(&payload).context("marshal task context marker")?;
    if let Err(err) = record_write_file(&marker_path, &data, manifest.as_deref_mut()) {
        if is_pre_exists(&err) {
            let existing = std::fs::read_to_string(&marker_path)
                .map_err(|e| anyhow::Error::new(e).context("read existing task context marker"))?;
            let parsed = serde_json::from_str::<TaskContextMarkerFile>(&existing);
            if parsed
                .map(|m| m.managed_by != TASK_CONTEXT_MARKER_MANAGED_BY)
                .unwrap_or(true)
            {
                return Err(err.context("write task context marker"));
            }
            std::fs::write(&marker_path, &data)
                .map_err(|e| anyhow::Error::new(e).context("refresh task context marker"))?;
            if let Some(m) = manifest {
                m.files.push(marker_path);
            }
            return Ok(());
        }
        return Err(err.context("write task context marker"));
    }
    Ok(())
}

/// The on-disk JSON written into the agent's working directory. Schema is
/// intentionally a thin pass-through of the API response.
#[derive(Debug, Clone, Default, Serialize)]
struct ProjectResourceFile {
    #[serde(rename = "project_id", skip_serializing_if = "String::is_empty")]
    project_id: String,
    #[serde(rename = "project_title", skip_serializing_if = "String::is_empty")]
    project_title: String,
    #[serde(
        rename = "project_description",
        skip_serializing_if = "String::is_empty"
    )]
    project_description: String,
    #[serde(rename = "resources")]
    resources: Vec<ProjectResourceForEnv>,
}

/// Writes .cordy/project/resources.json into the working directory when the
/// task carries project context. The file is always written when a project is
/// attached (even with zero resources) so agents can rely on its presence as a
/// signal that a project exists.
fn write_project_resources(
    work_dir: &str,
    ctx: &TaskContextForEnv,
    mut manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<()> {
    if ctx.project_id.is_empty() && ctx.project_resources.is_empty() {
        return Ok(());
    }
    let dir = join_path(&[work_dir, ".cordy", "project"]);
    record_mkdir_all(&dir, manifest.as_deref_mut())?;
    let payload = ProjectResourceFile {
        project_id: ctx.project_id.clone(),
        project_title: ctx.project_title.clone(),
        project_description: ctx.project_description.clone(),
        resources: ctx.project_resources.clone(),
    };
    let data = serde_json::to_vec_pretty(&payload)?;
    let path = join_path(&[&dir, "resources.json"]);
    if let Err(err) = record_write_file(&path, &data, manifest) {
        // .cordy/project/resources.json is Cordy-owned and a pre-existing path
        // is almost certainly user content the manifest must not destroy.
        if !is_pre_exists(&err) {
            return Err(err);
        }
    }
    Ok(())
}

/// Returns the directory where skills should be written based on the agent
/// provider, creating it.
fn resolve_skills_dir(
    work_dir: &str,
    provider: &str,
    manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<String> {
    let skills_dir = skills_dir_path(work_dir, provider);
    record_mkdir_all(&skills_dir, manifest)?;
    Ok(skills_dir)
}

// `pkg/agent.BuiltinRuntimeByID` is the canonical descriptor registry in Rust
// too; keep the execenv projection derived from it so new runtime identities
// inherit their native skills directory without another daemon-side map.
fn builtin_runtime_skills_dir(provider: &str) -> Option<&'static str> {
    cordy_agent::builtin_runtime(provider).map(|runtime| runtime.skills_dir)
}

/// Returns the provider-native skills parent directory under work_dir WITHOUT
/// creating it or recording anything. resolve_skills_dir wraps this with the
/// MkdirAll/manifest bookkeeping; the reuse-path skill rollback needs the bare
/// path with no side effects.
pub(crate) fn skills_dir_path(work_dir: &str, provider: &str) -> String {
    // Built-in runtime identities (e.g. "omp") declare their skills dir in
    // the descriptor; resolve generically before the protocol-family switch.
    if let Some(desc) = builtin_runtime_skills_dir(provider) {
        return join_path(&[work_dir, desc]);
    }
    match provider {
        "claude" => join_path(&[work_dir, ".claude", "skills"]),
        "codebuddy" => join_path(&[work_dir, ".codebuddy", "skills"]),
        "copilot" => join_path(&[work_dir, ".github", "skills"]),
        "opencode" => join_path(&[work_dir, ".opencode", "skills"]),
        "deveco" => join_path(&[work_dir, ".deveco", "skills"]),
        "openclaw" => join_path(&[work_dir, "skills"]),
        "pi" => join_path(&[work_dir, ".pi", "skills"]),
        "cursor" => join_path(&[work_dir, ".cursor", "skills"]),
        "kimi" => join_path(&[work_dir, ".kimi", "skills"]),
        "reasonix" => join_path(&[work_dir, ".reasonix", "skills"]),
        "dsh" => join_path(&[work_dir, ".dsh", "skills"]),
        "kiro" => join_path(&[work_dir, ".kiro", "skills"]),
        "qoder" | "qoderclicn" => join_path(&[work_dir, ".qoder", "skills"]),
        "qwen" => join_path(&[work_dir, ".qwen", "skills"]),
        "qwenpaw" => join_path(&[work_dir, "skill_pool"]),
        "mcode" => join_path(&[work_dir, ".minimax", "skills"]),
        "traecli" => join_path(&[work_dir, ".traecli", "skills"]),
        "antigravity" => join_path(&[work_dir, ".agents", "skills"]),
        "grok" => join_path(&[work_dir, ".grok", "skills"]),
        // Fallback: write to .agent_context/skills/ (referenced by meta config).
        _ => join_path(&[work_dir, ".agent_context", "skills"]),
    }
}

// S9-integration: internal/skill.IsReservedContentPath (reserved.go). The
// canonical filename check travels with the skill-package port; the rule is
// EqualFold(Clean(p), "SKILL.md").
fn is_reserved_content_path(p: &str) -> bool {
    clean_path(p).eq_ignore_ascii_case("SKILL.md")
}

// ---------------------------------------------------------------------------
// SKILL.md frontmatter handling
// ---------------------------------------------------------------------------

fn non_alpha_num_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^a-z0-9]+").expect("static regex"))
}

/// Converts a skill name to a safe directory name.
pub(crate) fn sanitize_skill_name(name: &str) -> String {
    let s = name.trim().to_lowercase();
    let s = non_alpha_num_re().replace_all(&s, "-");
    let s = s.trim_matches('-');
    if s.is_empty() {
        return "skill".to_string();
    }
    s.to_string()
}

/// Returns SKILL.md content guaranteed to lead with a YAML frontmatter block
/// carrying a parseable, non-empty `name` key. Runtimes like OpenCode silently
/// drop SKILL.md whose frontmatter is missing or whose `name` doesn't parse.
///
/// `name` is the one key Cordy must own: runtimes disagree on which field
/// identifies a skill (Claude routes on the directory name, OpenCode on the
/// frontmatter `name`), so letting the two diverge gives a single skill two
/// different invocable names depending on where it runs (MUL-5529).
pub(crate) fn ensure_skill_frontmatter(content: &str, slug: &str, description: &str) -> String {
    let Some(fm_start) = frontmatter_body_start(content) else {
        return synthesize_frontmatter(content, slug, description);
    };
    if is_frontmatter_valid_yaml(content) {
        // The parser, not the spelling, decides whether a name exists. A
        // lexical scan only recognizes a bare `name:` with a value on the same
        // line, so `"name": x` reads as nameless and would earn an injected
        // duplicate key that strict loaders reject outright.
        if frontmatter_has_name_key(content) {
            if let Some(rewritten) = set_frontmatter_name(content, fm_start, slug) {
                return rewritten;
            }
            // The surgical rewrite could not be proven correct — rebuild the
            // block from the parsed node instead of re-synthesizing: that
            // keeps every other key, including policy fields like
            // disable-model-invocation whose loss would re-expose a skill the
            // author hid from model invocation.
            if let Some(rebuilt) = rename_frontmatter_name_via_node(content, slug) {
                return rebuilt;
            }
            let (_, body, _) = frontmatter_parts(content);
            return synthesize_frontmatter(&body, slug, description);
        }
        // No name key at all, confirmed by the parser rather than inferred.
        // Inject one as the first key and keep the rest verbatim.
        return format!(
            "{}name: {}\n{}",
            &content[..fm_start],
            slug,
            &content[fm_start..]
        );
    }

    // Invalid YAML: fall back to the lexical scan. A block with a name is
    // stripped and re-synthesized so runtimes like Codex don't hard-reject
    // the whole skill at load time; frontmatter_parts returns the full
    // content as the body when it can't find a closing delimiter, so the
    // malformed block is kept rather than silently dropped.
    if has_frontmatter_name(&content[fm_start..]) {
        let (_, body, _) = frontmatter_parts(content);
        return synthesize_frontmatter(&body, slug, description);
    }
    format!(
        "{}name: {}\n{}",
        &content[..fm_start],
        slug,
        &content[fm_start..]
    )
}

/// Reports whether content's frontmatter parses as a mapping carrying a
/// top-level `name` key, whatever its spelling or value. Quoting is syntax,
/// not identity: `"name":` and `name:` are the same key, and a key with an
/// empty value is still the key. Only nesting makes a difference.
fn frontmatter_has_name_key(content: &str) -> bool {
    let (Some(fm_body), _, true) = frontmatter_parts(content) else {
        return false;
    };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&fm_body) else {
        return false;
    };
    let Some(mapping) = doc.as_mapping() else {
        return false;
    };
    mapping.contains_key(serde_yaml::Value::String("name".to_string()))
}

/// Produces a SKILL.md body with a YAML frontmatter block carrying at least
/// `name` and (when non-empty) `description`. The description is always
/// escaped as a double-quoted YAML string so values containing colons,
/// brackets, or other YAML-significant characters parse safely.
fn synthesize_frontmatter(body: &str, slug: &str, description: &str) -> String {
    let mut b = String::new();
    b.push_str("---\n");
    b.push_str(&format!("name: {slug}\n"));
    let d = description.trim();
    if !d.is_empty() {
        b.push_str(&format!("description: {}\n", yaml_escape_inline(d)));
    }
    b.push_str("---\n\n");
    b.push_str(body);
    b
}

/// Reports whether the opening YAML frontmatter block of content parses as a
/// YAML mapping. Returns false when there is no frontmatter, the block has no
/// closing delimiter, is empty, or unmarshalling fails.
fn is_frontmatter_valid_yaml(content: &str) -> bool {
    let (Some(fm_body), _, true) = frontmatter_parts(content) else {
        return false;
    };
    if fm_body.trim().is_empty() {
        return false;
    }
    serde_yaml::from_str::<serde_yaml::Mapping>(&fm_body).is_ok()
}

/// Splits content into the raw YAML frontmatter body (the text between the
/// opening `---` line and the closing `---` line) and the document body that
/// follows the closing delimiter. ok is false when content has no opening
/// delimiter or no closing delimiter line; in that case body is the full
/// content so callers can keep a malformed block instead of dropping it.
///
/// A closing delimiter is a line whose only content is `---`, terminated by
/// `\n`, `\r\n`, or end-of-file.
pub(crate) fn frontmatter_parts(content: &str) -> (Option<String>, String, bool) {
    let Some(start) = frontmatter_body_start(content) else {
        return (None, content.to_string(), false);
    };
    let rest = &content[start..];
    let bytes = rest.as_bytes();
    let mut search_from = 0usize;
    loop {
        let Some(nl) = rest[search_from..].find("\n---") else {
            // No closing delimiter line. An unterminated final line is still a
            // valid block ("terminated by \n, \r\n, or end-of-file").
            return if search_from == 0 && !rest.is_empty() {
                let trimmed = rest.strip_suffix('\n').unwrap_or(rest);
                (Some(trimmed.to_string()), String::new(), true)
            } else {
                (None, content.to_string(), false)
            };
        };
        let nl_at = search_from + nl;
        // fm body includes the newline terminating the last content line,
        // matching Go's line-scanner semantics.
        let close_at = nl_at;
        let after = &rest[nl_at + 1..]; // past the '\n', at "---..."
        let after = &after[3..]; // past '---'
        if after.is_empty() || after == "\r" {
            return (Some(rest[..close_at + 1].to_string()), String::new(), true);
        }
        if let Some(stripped) = after.strip_prefix('\n') {
            return (
                Some(rest[..close_at + 1].to_string()),
                stripped.to_string(),
                true,
            );
        }
        if let Some(stripped) = after.strip_prefix("\r\n") {
            return (
                Some(rest[..close_at + 1].to_string()),
                stripped.to_string(),
                true,
            );
        }
        // Not a standalone delimiter line (e.g. "----" or "--- text"); keep
        // scanning for the real close.
        search_from = close_at + 4;
        let _ = bytes;
    }
}

/// Returns the byte offset where the YAML body begins (just after the opening
/// `---` line) and whether a valid opening delimiter was found.
fn frontmatter_body_start(content: &str) -> Option<usize> {
    if let Some(stripped) = content.strip_prefix("---\n") {
        let _ = stripped;
        return Some(4);
    }
    if content.starts_with("---\r\n") {
        return Some(5);
    }
    None
}

/// Reports whether the frontmatter body contains a top-level `name` key before
/// the closing `---`, whatever its spelling or value. This is the malformed-
/// block twin of frontmatter_has_name_key: it answers the same question for
/// blocks that do not parse. Only unindented keys count.
fn has_frontmatter_name(fm_body: &str) -> bool {
    let close_idx = fm_body.find("\n---").unwrap_or(fm_body.len());
    for line in fm_body[..close_idx].split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("name:") || line.starts_with("\"name\":") || line.starts_with("'name':")
        {
            return true;
        }
    }
    false
}

/// Returns the byte range of the whole top-level `name` entry — key plus
/// value, however many lines the value occupies — inside a frontmatter body.
///
/// Deviation vs Go: Go derives the extent from yaml.v3's own line numbers.
/// serde_yaml exposes no source positions, so this scans lexically for the
/// next unindented mapping-key line. Callers verify the rewritten result by
/// re-parsing (frontmatter_name_is) and fall back to the node rebuild on any
/// mismatch, so an over-broad span degrades safely.
fn frontmatter_name_value_span(fm_body: &str) -> Option<(usize, usize)> {
    let close_idx = fm_body.find("\n---").unwrap_or(fm_body.len());
    let block = &fm_body[..close_idx];

    let lines: Vec<&str> = block.split('\n').collect();
    let mut name_line = None;
    for (i, line) in lines.iter().enumerate() {
        if is_top_level_name_line(line) {
            name_line = Some(i);
            break;
        }
    }
    let name_line = name_line?;

    // End at the next unindented mapping-key line after the name entry.
    let mut last_line = lines.len();
    for (i, line) in lines.iter().enumerate().skip(name_line + 1) {
        if looks_like_top_level_key(line) {
            last_line = i;
            break;
        }
    }
    // Blank lines and unindented comments sitting between this entry and the
    // next key are not part of the value: leave them in place instead of
    // swallowing them into the replacement.
    while last_line > name_line + 1 {
        let line = lines[last_line - 1]
            .strip_suffix('\r')
            .unwrap_or(lines[last_line - 1]);
        if line.trim().is_empty() || line.starts_with('#') {
            last_line -= 1;
            continue;
        }
        break;
    }
    if last_line <= name_line {
        return None;
    }

    let mut offset = 0usize;
    let mut start = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if i == name_line {
            start = offset;
        }
        if i == last_line - 1 {
            return Some((start, offset + line.len()));
        }
        offset += line.len() + 1; // +1 for the newline split consumed
    }
    None
}

fn is_top_level_name_line(line: &str) -> bool {
    line.starts_with("name:") || line.starts_with("\"name\":") || line.starts_with("'name':")
}

/// Conservative lexical test for "this line starts a new top-level mapping
/// key": unindented, not a comment, and carries a `key:` shape before any
/// inline value. Continuation lines of multi-line scalars (indented, or
/// colon-less prose) do not match.
fn looks_like_top_level_key(line: &str) -> bool {
    let trimmed_end = line.strip_suffix('\r').unwrap_or(line);
    if trimmed_end.is_empty()
        || trimmed_starts_with_space_or_tab(trimmed_end)
        || trimmed_end.starts_with('#')
    {
        return false;
    }
    let Some(colon) = trimmed_end.find(':') else {
        return false;
    };
    if colon == 0 {
        return false;
    }
    let after = &trimmed_end[colon + 1..];
    after.is_empty() || after.starts_with(' ') || after.starts_with('\t')
}

fn trimmed_starts_with_space_or_tab(s: &str) -> bool {
    s.starts_with(' ') || s.starts_with('\t')
}

/// Replaces the top-level frontmatter `name` entry — key and full value,
/// however many lines it spans — with a single-line `name: <slug>`, leaving
/// every other byte of content untouched. fmStart is the offset where the YAML
/// body begins. Returns None when the result cannot be proven correct: the
/// directory == frontmatter-name invariant is the entire reason this function
/// exists, so a rewrite that cannot prove it is worse than no rewrite at all.
fn set_frontmatter_name(content: &str, fm_start: usize, slug: &str) -> Option<String> {
    let fm_body = &content[fm_start..];
    let (start, end) = frontmatter_name_value_span(fm_body)?;
    let mut replacement = format!("name: {slug}");
    // strings.Split on "\n" leaves the "\r" of a CRLF ending inside the line;
    // carry it over so the block doesn't end up with mixed terminators.
    if fm_body[start..end].ends_with('\r') {
        replacement.push('\r');
    }
    let rewritten = format!(
        "{}{}{}",
        &content[..fm_start + start],
        replacement,
        &content[fm_start + end..]
    );
    if !frontmatter_name_is(&rewritten, slug) {
        return None;
    }
    Some(rewritten)
}

/// Rebuilds the frontmatter block from its parsed YAML node with `name` set to
/// slug, and returns the reassembled document.
///
/// This is the middle ground between the surgical byte rewrite and full
/// re-synthesis. It gives up the original block's exact formatting — the
/// re-marshal normalizes quoting and indentation — but keeps every key and
/// its value, which re-synthesis does not.
///
/// Anchors/aliases: serde_yaml resolves aliases to their values at parse time
/// and drops anchors on re-serialize, which is precisely the outcome Go's
/// materializeAliasesOf pass produces before rewriting the anchor away.
fn rename_frontmatter_name_via_node(content: &str, slug: &str) -> Option<String> {
    let (fm_body, body, ok) = frontmatter_parts(content);
    if !ok {
        return None;
    }
    let fm_body = fm_body?;
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&fm_body).ok()?;
    let mapping = doc.as_mapping_mut()?;
    let key = serde_yaml::Value::String("name".to_string());
    if !mapping.contains_key(&key) {
        return None;
    }
    mapping.insert(key, serde_yaml::Value::String(slug.to_string()));

    let mut marshaled = serde_yaml::to_string(&doc).ok()?;
    if !marshaled.ends_with('\n') {
        marshaled.push('\n');
    }
    let rebuilt = format!("---\n{marshaled}---\n{body}");
    if !frontmatter_name_is(&rebuilt, slug) {
        return None;
    }
    Some(rebuilt)
}

/// Reports whether content's frontmatter parses and its top-level `name` is
/// exactly want.
fn frontmatter_name_is(content: &str, want: &str) -> bool {
    let (Some(fm_body), _, true) = frontmatter_parts(content) else {
        return false;
    };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&fm_body) else {
        return false;
    };
    doc.get("name").and_then(|v| v.as_str()) == Some(want)
}

/// Returns a double-quoted YAML scalar that always parses as a string. Plain
/// scalars are deliberately avoided: values like `[foo]`, `{x: y}`, `false`,
/// `null`, or `2024-01-01` would parse as flow sequences, flow mappings,
/// booleans, nulls, or timestamps under YAML 1.2. Newlines are flattened
/// (frontmatter values are single-line per key) and `\` and `"` escaped.
// Sequential replaces mirror Go byte-for-byte: "\r\n" collapses to ONE space
// first; a single-pass char-class replace would emit two.
#[allow(clippy::collapsible_str_replace)]
fn yaml_escape_inline(s: &str) -> String {
    let flat = s.replace("\r\n", " ").replace('\n', " ").replace('\r', " ");
    let escaped = flat.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

// ---------------------------------------------------------------------------
// Skill writing
// ---------------------------------------------------------------------------

/// Assigns each skill in a batch its on-disk directory slug, deduplicating
/// within the batch so two skills never claim the same directory (skill_
/// visibility.go). sanitize_skill_name alone is not injective: "A B" and "A-B"
/// both reduce to "a-b". Deterministic in batch index order, so the brief
/// stays byte-identical across runs for identical input.
pub(crate) fn resolve_skill_slugs(skills: &[SkillContextForEnv]) -> Vec<String> {
    let mut slugs = Vec::with_capacity(skills.len());
    let mut taken: HashMap<String, ()> = HashMap::with_capacity(skills.len());
    for skill in skills {
        let base = sanitize_skill_name(&skill.name);
        let mut slug = base.clone();
        let mut attempt = 1usize;
        loop {
            if !taken.contains_key(&slug) {
                break;
            }
            slug = skill_slug_candidate(&base, attempt);
            attempt += 1;
        }
        taken.insert(slug.clone(), ());
        slugs.push(slug);
    }
    slugs
}

/// Writes skill directories into the given parent directory. Each skill gets
/// its own subdirectory containing SKILL.md and supporting files. When a
/// Cordy skill's natural slug collides with a user-installed skill at the same
/// path, we allocate a collision-free sibling slug (e.g. `issue-review-cordy`)
/// and write there instead — the user's original directory stays bit-for-bit
/// intact (PR #3444).
pub(crate) fn write_skill_files(
    skills_dir: &str,
    skills: &[SkillContextForEnv],
    mut manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<()> {
    record_mkdir_all(skills_dir, manifest.as_deref_mut()).context("create skills dir")?;

    // resolve_skill_slugs deduplicates within the batch first, so two skills
    // whose names sanitize alike get distinct bases instead of racing for the
    // same directory. allocate_collision_free_skill_dir still runs on top, for
    // collisions against directories we did not write (user-installed skills).
    let batch_slugs = resolve_skill_slugs(skills);

    for (i, skill) in skills.iter().enumerate() {
        let (slug, dir) = allocate_collision_free_skill_dir(skills_dir, &batch_slugs[i])
            .with_context(|| format!("allocate skill dir for {:?}", skill.name))?;
        record_mkdir_all(&dir, manifest.as_deref_mut())?;

        // ensure_skill_frontmatter synthesises a `name:` value when the
        // upstream skill is missing one. Use the chosen slug (which may differ
        // from baseSlug on collision) so the YAML name matches the directory
        // name; runtimes that key on either stay consistent.
        let body = ensure_skill_frontmatter(&skill.content, &slug, &skill.description);
        record_write_file(
            &join_path(&[&dir, "SKILL.md"]),
            body.as_bytes(),
            manifest.as_deref_mut(),
        )?;

        // Write supporting files. The skill directory is collision-free by
        // construction, so a record_write_file collision under it would mean
        // the skill's bundled files list two entries at the same path — an
        // upstream data bug, surfaced as such. One common data bug is storing
        // SKILL.md as both primary content and a supporting file; the check is
        // canonical so "./SKILL.md" is caught too.
        for f in &skill.files {
            if is_reserved_content_path(&f.path) {
                continue;
            }
            let fpath = join_path(&[&dir, &f.path]);
            record_mkdir_all(&dir_of(&fpath), manifest.as_deref_mut())?;
            record_write_file(&fpath, f.content.as_bytes(), manifest.as_deref_mut())?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// issue_context.md rendering
// ---------------------------------------------------------------------------

/// Builds the markdown content for issue_context.md.
pub(crate) fn render_issue_context(ctx: &TaskContextForEnv) -> String {
    if !ctx.autopilot_run_id.is_empty() {
        return render_autopilot_context(ctx);
    }
    if !ctx.quick_create_prompt.is_empty() {
        return render_quick_create_context(ctx);
    }

    let mut b = String::new();

    b.push_str("# Task Assignment\n\n");
    b.push_str(&format!("**Issue ID:** {}\n\n", ctx.issue_id));

    if !ctx.trigger_comment_id.is_empty() {
        b.push_str("**Trigger:** Comment Reply\n");
        b.push_str(&format!(
            "**Triggering comment ID:** `{}`\n\n",
            ctx.trigger_comment_id
        ));
    } else {
        b.push_str("**Trigger:** New Assignment\n\n");
    }

    // Assignment handoff note (MUL-3375): the assigner's scoping instruction
    // for this run. Distinct from a comment — there is no thread to reply to.
    if !ctx.handoff_note.is_empty() {
        b.push_str("## Handoff Note\n\n");
        b.push_str("The person who assigned this issue left this instruction for the run. Treat it as scope guidance and follow it before doing anything broader:\n\n");
        b.push_str(&format!("> {}\n\n", ctx.handoff_note));
    }

    b.push_str("## Quick Start\n\n");
    b.push_str(&format!(
        "Run `cordy issue get {} --output json` to fetch the full issue details.\n\n",
        ctx.issue_id
    ));

    b
}

/// Renders issue_context.md for quick-create tasks. This file carries only
/// task data (the user input); behavioral rules live in AGENTS.md and the
/// per-turn prompt (MUL-5529).
fn render_quick_create_context(ctx: &TaskContextForEnv) -> String {
    let mut b = String::new();
    b.push_str("# Quick Create\n\n");
    b.push_str("**Trigger:** Quick-create modal\n\n");
    b.push_str("## User input\n\n");
    b.push_str("> ");
    b.push_str(&ctx.quick_create_prompt);
    b.push_str("\n\n");
    b
}

fn render_autopilot_context(ctx: &TaskContextForEnv) -> String {
    let mut b = String::new();

    b.push_str("# Autopilot Run\n\n");
    b.push_str(&format!(
        "**Autopilot run ID:** {}\n\n",
        ctx.autopilot_run_id
    ));
    if !ctx.autopilot_id.is_empty() {
        b.push_str(&format!("**Autopilot ID:** {}\n\n", ctx.autopilot_id));
    }
    if !ctx.autopilot_title.is_empty() {
        b.push_str(&format!("**Title:** {}\n\n", ctx.autopilot_title));
    }
    if !ctx.autopilot_source.is_empty() {
        b.push_str(&format!("**Trigger source:** {}\n\n", ctx.autopilot_source));
    }
    if !ctx.autopilot_trigger_payload.is_empty() {
        b.push_str(&format!(
            "## Trigger Payload\n\n```json\n{}\n```\n\n",
            ctx.autopilot_trigger_payload
        ));
    }

    b.push_str("## Quick Start\n\n");
    b.push_str("This is a run-only autopilot task with no assigned issue. Do not run `cordy issue get` unless the autopilot instructions explicitly ask you to create or update an issue.\n\n");
    if !ctx.autopilot_id.is_empty() {
        b.push_str(&format!(
            "Run `cordy autopilot get {} --output json` if you need the full autopilot configuration.\n\n",
            ctx.autopilot_id
        ));
    }
    if !ctx.autopilot_description.trim().is_empty() {
        b.push_str("## Autopilot Instructions\n\n");
        b.push_str(&ctx.autopilot_description);
        b.push_str("\n\n");
    }

    b
}

// ---------------------------------------------------------------------------
// Runtime skill policy (hosted from runtime_skill_policy.go)
// ---------------------------------------------------------------------------

const CLAUDE_RUNTIME_SKILL_SETTINGS_FILE: &str = "claude-runtime-skill-settings.json";

/// Identifies a runtime-local skill for provider-specific task environment
/// filtering. Provider and runtime are already selected by the task, so only
/// the discovery root and provider-native key are needed here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RuntimeSkillRefForEnv {
    pub root: String,
    pub key: String,
    pub name: String,
    pub plugin: String,
}

fn clean_runtime_skill_key(key: &str) -> Option<String> {
    let cleaned = clean_path(key.trim());
    if cleaned == "." || cleaned.starts_with('/') || cleaned == ".." || cleaned.starts_with("../") {
        return None;
    }
    Some(cleaned)
}

/// Prepares a task-local --settings JSON applying disabled runtime-skill
/// policy without mutating the user's Claude config. Returns the settings path,
/// or "" when nothing needed to be written (and any stale file was removed).
pub(crate) fn prepare_claude_skill_settings(
    env_root: &str,
    disabled: &[RuntimeSkillRefForEnv],
    workspace_skills: &[SkillContextForEnv],
) -> anyhow::Result<String> {
    let path = join_path(&[env_root, CLAUDE_RUNTIME_SKILL_SETTINGS_FILE]);
    if disabled.is_empty() {
        remove_file_tolerating_absence(&path)?;
        return Ok(String::new());
    }

    let mut overrides: std::collections::BTreeMap<String, String> = Default::default();
    let mut deny: Vec<String> = Vec::with_capacity(disabled.len() * 2);
    let mut seen_deny: std::collections::HashSet<String> = std::collections::HashSet::new();
    let add_deny =
        |rule: String, seen: &mut std::collections::HashSet<String>, deny: &mut Vec<String>| {
            if seen.insert(rule.clone()) {
                deny.push(rule);
            }
        };
    for skill in disabled {
        let Some(key) = clean_runtime_skill_key(&skill.key) else {
            continue;
        };
        let mut invocation_name = skill.name.trim().to_string();
        if invocation_name.is_empty() {
            invocation_name = basename(&key);
        }
        if workspace_claims_runtime_skill(&invocation_name, workspace_skills) {
            continue;
        }
        // Claude Code's skillOverrides fully hides personal/project skills.
        // Plugin skills ignore that setting, so the permission deny below is
        // also emitted for every key and is the enforcement path for plugins.
        if skill.root != "plugin" {
            overrides.insert(invocation_name.clone(), "off".to_string());
        } else {
            invocation_name = key.clone();
        }
        add_deny(
            format!("Skill({invocation_name})"),
            &mut seen_deny,
            &mut deny,
        );
        add_deny(
            format!("Skill({invocation_name} *)"),
            &mut seen_deny,
            &mut deny,
        );
    }
    if overrides.is_empty() && deny.is_empty() {
        remove_file_tolerating_absence(&path)?;
        return Ok(String::new());
    }
    // Compact JSON like Go's json.Marshal; BTreeMap ordering matches Go's
    // sorted map keys.
    let payload = serde_json::json!({
        "skillOverrides": overrides,
        "permissions": { "deny": deny },
    });
    let data = serde_json::to_vec(&payload)?;
    std::fs::write(&path, data)?;
    Ok(path)
}

fn remove_file_tolerating_absence(path: &str) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

fn basename(path: &str) -> String {
    let cleaned = clean_path(path);
    match cleaned.rfind('/') {
        Some(i) => cleaned[i + 1..].to_string(),
        None => cleaned,
    }
}

/// Appends `[[skills.config]] enabled=false` blocks for disabled Codex skills
/// to the per-task config.toml.
pub(crate) fn ensure_codex_disabled_skills_config(
    config_path: &str,
    codex_home: &str,
    disabled: &[RuntimeSkillRefForEnv],
    workspace_skills: &[SkillContextForEnv],
) -> anyhow::Result<()> {
    if disabled.is_empty() {
        return Ok(());
    }
    let mut home = String::new();
    let mut paths: Vec<String> = Vec::with_capacity(disabled.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for skill in disabled {
        let Some(key) = clean_runtime_skill_key(&skill.key) else {
            continue;
        };
        let skill_path = match skill.root.as_str() {
            "provider" => {
                let first_key_part = key.split('/').next().unwrap_or("").to_string();
                if workspace_claims_runtime_skill(&first_key_part, workspace_skills) {
                    continue;
                }
                join_path(&[codex_home, "skills", &key, "SKILL.md"])
            }
            "universal" => {
                if home.is_empty() {
                    home = super::execenv::user_home_dir()
                        .context("resolve user home for disabled Codex skills")?;
                }
                join_path(&[&home, ".agents", "skills", &key, "SKILL.md"])
            }
            _ => continue,
        };
        if !seen.insert(skill_path.clone()) {
            continue;
        }
        paths.push(skill_path);
    }
    if paths.is_empty() {
        return Ok(());
    }
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config_path)?;
    for path in &paths {
        let block = format!(
            "\n[[skills.config]]\npath = {}\nenabled = false\n",
            go_quote(path)
        );
        file.write_all(block.as_bytes())?;
    }
    Ok(())
}

fn workspace_claims_runtime_skill(name: &str, workspace_skills: &[SkillContextForEnv]) -> bool {
    let claim = sanitize_skill_name(name);
    workspace_skills
        .iter()
        .any(|s| sanitize_skill_name(&s.name) == claim)
}

/// strconv.Quote subset covering filesystem paths: double quotes, backslash/
/// quote/newline/tab escapes, ASCII control characters as \xNN, printable
/// non-ASCII kept verbatim (Go keeps IsPrint runes too).
pub(crate) fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// dir_of ports Go filepath.Dir: everything up to the final separator,
/// Cleaned; "." when there is no separator.
pub(crate) fn dir_of(path: &str) -> String {
    let cleaned = clean_path(path);
    match cleaned.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => clean_path(&cleaned[..i]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestEnsureWorkspacesRootMarker (context_marker_test.go core
    // cases): fresh write, idempotent re-write, foreign refusal, torn-write
    // reclaim.
    #[test]
    fn test_workspaces_root_marker_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        ensure_workspaces_root_marker(&root).unwrap();
        let path = join_path(&[&root, TASK_CONTEXT_MARKER_REL_PATH]);
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains(TASK_CONTEXT_MARKER_MANAGED_BY));

        // Idempotent: owned marker left untouched.
        ensure_workspaces_root_marker(&root).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        // Foreign but parseable: refused.
        std::fs::write(&path, br#"{"managed_by":"someone-else"}"#).unwrap();
        let err = ensure_workspaces_root_marker(&root).unwrap_err();
        assert!(format!("{err:#}").contains("refusing to overwrite"));

        // Torn write: reclaimed.
        std::fs::write(&path, b"{trunc").unwrap();
        ensure_workspaces_root_marker(&root).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        // Empty root refused.
        assert!(ensure_workspaces_root_marker("  ").is_err());
    }

    // Port of TestFrontmatterParts (skill_frontmatter_test.go cases).
    #[test]
    fn test_frontmatter_parts() {
        let (fm, body, ok) = frontmatter_parts("---\nname: x\n---\nbody");
        assert!(ok);
        assert_eq!(fm.unwrap(), "name: x\n");
        assert_eq!(body, "body");

        // CRLF close.
        let (fm, body, ok) = frontmatter_parts("---\nname: x\r\n---\r\nbody");
        assert!(ok);
        assert_eq!(fm.unwrap(), "name: x\r\n");
        assert_eq!(body, "body");

        // EOF close.
        let (fm, body, ok) = frontmatter_parts("---\nname: x");
        assert!(ok);
        assert_eq!(fm.unwrap(), "name: x");
        assert_eq!(body, "");

        // "----" is not a delimiter.
        let (fm, _, ok) = frontmatter_parts("---\nname: x\n----\nafter");
        assert!(!ok);
        assert!(fm.is_none());

        // No opening delimiter.
        let (_, body, ok) = frontmatter_parts("plain text");
        assert!(!ok);
        assert_eq!(body, "plain text");

        // "--- text" inside is skipped over.
        let (fm, body, ok) = frontmatter_parts("---\ndesc: --- text\n---\nreal");
        assert!(ok);
        assert_eq!(fm.unwrap(), "desc: --- text\n");
        assert_eq!(body, "real");
    }

    // Port of TestEnsureSkillFrontmatter (skill_frontmatter_test.go cases).
    #[test]
    fn test_ensure_skill_frontmatter_synthesizes_when_missing() {
        let got = ensure_skill_frontmatter("# Body\n", "my-slug", "Does things: well");
        assert_eq!(
            got,
            "---\nname: my-slug\ndescription: \"Does things: well\"\n---\n\n# Body\n"
        );

        // Existing valid frontmatter without a name gets one injected first.
        let got = ensure_skill_frontmatter("---\ndescription: d\n---\nBody", "s2", "");
        assert_eq!(got, "---\nname: s2\ndescription: d\n---\nBody");

        // Existing name is rewritten to the slug, other keys preserved.
        let got = ensure_skill_frontmatter(
            "---\nname: old\ndescription: keep\n---\nBody",
            "new-name",
            "",
        );
        assert_eq!(got, "---\nname: new-name\ndescription: keep\n---\nBody");

        // Multi-line quoted name value is fully replaced.
        let got = ensure_skill_frontmatter(
            "---\nname: \"wrapped\ncontinued\"\ndescription: d\n---\nBody",
            "slug",
            "",
        );
        assert!(got.starts_with("---\nname: slug\n"), "got: {got}");
        assert!(got.contains("description: d"), "got: {got}");

        // Invalid YAML with a name is stripped and re-synthesized.
        let got = ensure_skill_frontmatter("---\nname: broken: [oops\n---\nBody", "fixed", "d");
        assert_eq!(got, "---\nname: fixed\ndescription: \"d\"\n---\n\nBody");

        // Invalid YAML without a name gets one injected.
        let got = ensure_skill_frontmatter("---\ndescription: [oops\n---\nBody", "fixed", "");
        assert!(got.starts_with("---\nname: fixed\n"), "got: {got}");
        assert!(got.contains("description: [oops"), "malformed block kept");
    }

    // Port of TestFrontmatterHasNameKey: quoting is syntax, not identity;
    // nested names don't count.
    #[test]
    fn test_frontmatter_has_name_key() {
        assert!(frontmatter_has_name_key("---\nname: x\n---\n"));
        assert!(frontmatter_has_name_key("---\n\"name\": x\n---\n"));
        assert!(frontmatter_has_name_key("---\n'name':\n---\n"));
        assert!(!frontmatter_has_name_key(
            "---\nmetadata:\n  name: x\n---\n"
        ));
        assert!(!frontmatter_has_name_key("---\ndescription: x\n---\n"));
        assert!(!frontmatter_has_name_key("no frontmatter"));
    }

    // Port of TestResolveSkillSlugs (skill_visibility_test.go): batch dedup
    // assigns distinct slugs deterministically.
    #[test]
    fn test_resolve_skill_slugs_dedups_within_batch() {
        let skills = vec![
            SkillContextForEnv {
                name: "A B".into(),
                ..Default::default()
            },
            SkillContextForEnv {
                name: "A-B".into(),
                ..Default::default()
            },
            SkillContextForEnv {
                name: "a b".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            resolve_skill_slugs(&skills),
            vec!["a-b", "a-b-cordy", "a-b-cordy-2"]
        );
        assert_eq!(resolve_skill_slugs(&[]), Vec::<String>::new());
    }

    // Port of TestWriteSkillFilesCollisionFallback (local_directory flows):
    // a user-owned directory at the natural slug forces the -cordy sibling.
    #[test]
    fn test_write_skill_files_collision_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let skills_dir = tmp.path().join("skills").to_string_lossy().to_string();
        std::fs::create_dir_all(join_path(&[&skills_dir, "review"])).unwrap();
        std::fs::write(join_path(&[&skills_dir, "review", "user.txt"]), b"user").unwrap();

        let skills = vec![SkillContextForEnv {
            name: "Review".into(),
            description: "d".into(),
            content: "# Review".into(),
            files: vec![SkillFileContextForEnvAlias {
                path: "scripts/run.sh".into(),
                content: "echo hi".into(),
            }],
        }];
        write_skill_files(&skills_dir, &skills, None).unwrap();

        // User directory untouched; skill landed in review-cordy.
        assert_eq!(
            std::fs::read_to_string(join_path(&[&skills_dir, "review", "user.txt"])).unwrap(),
            "user"
        );
        let skill_md =
            std::fs::read_to_string(join_path(&[&skills_dir, "review-cordy", "SKILL.md"])).unwrap();
        assert!(skill_md.starts_with("---\nname: review-cordy\n"));
        assert_eq!(
            std::fs::read_to_string(join_path(&[
                &skills_dir,
                "review-cordy",
                "scripts",
                "run.sh"
            ]))
            .unwrap(),
            "echo hi"
        );

        // Reserved content path (SKILL.md as a bundled file) is skipped rather
        // than colliding.
        let dup = vec![SkillContextForEnv {
            name: "Dup".into(),
            description: String::new(),
            content: "# Dup".into(),
            files: vec![SkillFileContextForEnvAlias {
                path: "./SKILL.md".into(),
                content: "duplicate".into(),
            }],
        }];
        write_skill_files(&skills_dir, &dup, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(join_path(&[&skills_dir, "dup", "SKILL.md"])).unwrap(),
            "---\nname: dup\n---\n\n# Dup"
        );
    }

    // Local alias so tests can build file entries without importing the
    // PascalCase wire struct spelling everywhere.
    type SkillFileContextForEnvAlias = super::super::execenv::SkillFileContextForEnv;

    // Port of TestRenderIssueContext variants.
    #[test]
    fn test_render_issue_context_variants() {
        let issue = TaskContextForEnv {
            issue_id: "iss_1".into(),
            trigger_comment_id: "cmt_9".into(),
            handoff_note: "keep it small".into(),
            ..Default::default()
        };
        let md = render_issue_context(&issue);
        assert!(md.starts_with("# Task Assignment\n\n"));
        assert!(md.contains("**Issue ID:** iss_1"));
        assert!(md.contains("**Trigger:** Comment Reply"));
        assert!(md.contains("`cmt_9`"));
        assert!(md.contains("> keep it small"));
        assert!(md.contains("cordy issue get iss_1 --output json"));

        let assigned = TaskContextForEnv {
            issue_id: "iss_2".into(),
            ..Default::default()
        };
        let md = render_issue_context(&assigned);
        assert!(md.contains("**Trigger:** New Assignment"));
        assert!(!md.contains("Handoff Note"));

        let qc = TaskContextForEnv {
            quick_create_prompt: "build a thing".into(),
            ..Default::default()
        };
        let md = render_issue_context(&qc);
        assert!(md.starts_with("# Quick Create\n"));
        assert!(md.contains("> build a thing"));

        let ap = TaskContextForEnv {
            autopilot_run_id: "run_1".into(),
            autopilot_id: "ap_1".into(),
            autopilot_title: "Nightly".into(),
            autopilot_source: "cron".into(),
            autopilot_trigger_payload: "{\"k\":1}".into(),
            autopilot_description: "Do the rounds".into(),
            ..Default::default()
        };
        let md = render_issue_context(&ap);
        assert!(md.starts_with("# Autopilot Run\n"));
        assert!(md.contains("**Autopilot run ID:** run_1"));
        assert!(md.contains("**Autopilot ID:** ap_1"));
        assert!(md.contains("**Title:** Nightly"));
        assert!(md.contains("**Trigger source:** cron"));
        assert!(md.contains("```json\n{\"k\":1}\n```"));
        assert!(md.contains("## Autopilot Instructions\n\nDo the rounds"));
        assert!(md.contains("cordy autopilot get ap_1 --output json"));
    }

    // Port of TestSkillsDirPath (provider table spot checks).
    #[test]
    fn test_skills_dir_paths() {
        assert_eq!(skills_dir_path("/w", "claude"), "/w/.claude/skills");
        assert_eq!(skills_dir_path("/w", "codebuddy"), "/w/.codebuddy/skills");
        assert_eq!(skills_dir_path("/w", "copilot"), "/w/.github/skills");
        assert_eq!(skills_dir_path("/w", "opencode"), "/w/.opencode/skills");
        assert_eq!(skills_dir_path("/w", "openclaw"), "/w/skills");
        assert_eq!(skills_dir_path("/w", "qwenpaw"), "/w/skill_pool");
        assert_eq!(skills_dir_path("/w", "omp"), "/w/.omp/skills");
        assert_eq!(skills_dir_path("/w", "unknown"), "/w/.agent_context/skills");
    }

    // Port of TestSanitizeSkillName.
    #[test]
    fn test_sanitize_skill_name() {
        assert_eq!(sanitize_skill_name("PR Review"), "pr-review");
        assert_eq!(sanitize_skill_name("  --odd--name--  "), "odd-name");
        assert_eq!(sanitize_skill_name(""), "skill");
        assert_eq!(sanitize_skill_name("!!!"), "skill");
    }

    // Port of TestCleanRuntimeSkillKey.
    #[test]
    fn test_clean_runtime_skill_key() {
        assert_eq!(clean_runtime_skill_key(" a/b "), Some("a/b".into()));
        assert_eq!(clean_runtime_skill_key("./a//b/./"), Some("a/b".into()));
        assert_eq!(clean_runtime_skill_key(".."), None);
        assert_eq!(clean_runtime_skill_key("../escape"), None);
        assert_eq!(clean_runtime_skill_key("/abs"), None);
        assert_eq!(clean_runtime_skill_key("."), None);
    }

    // Port of TestPrepareClaudeSkillSettings (runtime_skill_policy_test.go):
    // ordinary skill override, plugin routing to deny-only, stale-file removal.
    #[test]
    fn test_prepare_claude_skill_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        let path = prepare_claude_skill_settings(
            &root,
            &[
                RuntimeSkillRefForEnv {
                    root: "provider".into(),
                    key: "review-dir".into(),
                    name: "review".into(),
                    plugin: String::new(),
                },
                RuntimeSkillRefForEnv {
                    root: "plugin".into(),
                    key: "paper:design-to-code".into(),
                    name: String::new(),
                    plugin: "paper@market".into(),
                },
            ],
            &[],
        )
        .unwrap();
        assert!(!path.is_empty());
        let data = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(v["skillOverrides"]["review"], "off");
        assert!(v["skillOverrides"].get("paper:design-to-code").is_none());
        let deny = v["permissions"]["deny"].as_array().unwrap();
        for want in [
            "Skill(review)",
            "Skill(review *)",
            "Skill(paper:design-to-code)",
            "Skill(paper:design-to-code *)",
        ] {
            assert!(
                deny.contains(&serde_json::json!(want)),
                "missing {want} in {deny:?}"
            );
        }

        // Cleared → file removed and empty path returned.
        let cleared = prepare_claude_skill_settings(&root, &[], &[]).unwrap();
        assert_eq!(cleared, "");
        assert!(!Path::new(&path).exists());

        // Workspace claim skips the entry entirely.
        let disabled_claimed = vec![RuntimeSkillRefForEnv {
            root: "universal".into(),
            key: "mine".into(),
            name: "Mine".into(),
            plugin: String::new(),
        }];
        let workspace = vec![SkillContextForEnv {
            name: "mine".into(),
            ..Default::default()
        }];
        let p2 = prepare_claude_skill_settings(&root, &disabled_claimed, &workspace).unwrap();
        assert_eq!(p2, "", "fully-filtered batch removes the settings file");
    }

    // Port of TestGoQuoteShape (strconv.Quote subset used by the codex
    // disabled-skills config writer).
    #[test]
    fn test_go_quote() {
        assert_eq!(go_quote("/home/u/a b.md"), "\"/home/u/a b.md\"");
        assert_eq!(go_quote("q\"ote\\path"), "\"q\\\"ote\\\\path\"");
        assert_eq!(go_quote("nl\nhere"), "\"nl\\nhere\"");
        assert_eq!(go_quote("\u{7f}"), "\"\\x7f\"");
    }

    // Port of TestIsReservedContentPath canonical matching.
    #[test]
    fn test_is_reserved_content_path() {
        assert!(is_reserved_content_path("SKILL.md"));
        assert!(is_reserved_content_path("./SKILL.md"));
        assert!(is_reserved_content_path("sub/../SKILL.md"));
        assert!(is_reserved_content_path("skill.md"));
        assert!(!is_reserved_content_path("docs/SKILL.md.bak"));
    }

    // Port of TestSidecarManifestRollback semantics: ENOENT tolerated,
    // user-populated dirs preserved, real errors surfaced.
    #[test]
    fn test_sidecar_manifest_rollback_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let mut m = SidecarManifest::default();

        let dir_a = join_path(&[&root, "created"]);
        record_mkdir_all(&dir_a, Some(&mut m)).unwrap();
        let f = join_path(&[&dir_a, "file.txt"]);
        record_write_file(&f, b"x", Some(&mut m)).unwrap();
        assert_eq!(m.dirs, vec![dir_a.clone()]);
        assert_eq!(m.files, vec![f.clone()]);

        // Pre-existing path refused untouched.
        let pre = join_path(&[&root, "pre.txt"]);
        std::fs::write(&pre, b"user").unwrap();
        assert!(record_write_file(&pre, b"new", Some(&mut m)).is_err());
        assert_eq!(std::fs::read_to_string(&pre).unwrap(), "user");

        // Rollback removes our writes; user content survives.
        roll_back_prepared_sidecars(&m).unwrap();
        assert!(!std::path::Path::new(&f).exists());
        assert!(!std::path::Path::new(&dir_a).exists());
        assert_eq!(std::fs::read_to_string(&pre).unwrap(), "user");

        // Manifest persistence round-trip via cleanup_sidecars.
        let env_root = join_path(&[&root, "env"]);
        std::fs::create_dir_all(&env_root).unwrap();
        let mut m2 = SidecarManifest::default();
        let d2 = join_path(&[&root, "side"]);
        record_mkdir_all(&d2, Some(&mut m2)).unwrap();
        write_sidecar_manifest(&env_root, &m2).unwrap();
        cleanup_sidecars(&env_root).unwrap();
        assert!(!std::path::Path::new(&d2).exists());
        assert!(!Path::new(&env_root).join(SIDECAR_MANIFEST_FILE).exists());

        // Missing manifest is a no-op.
        cleanup_sidecars(&join_path(&[&root, "nope"])).unwrap();
    }

    // Port of TestRemoveReusedManagedSkillDirs: only direct children of the
    // skills parent are reclaimed, even when populated.
    #[test]
    fn test_remove_reused_managed_skill_dirs_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        let env_root = join_path(&[&root, "env"]);
        let skills_parent = join_path(&[&root, ".claude", "skills"]);

        std::fs::create_dir_all(&env_root).unwrap();
        let mut m = SidecarManifest::default();
        let managed = join_path(&[&skills_parent, "review"]);
        record_mkdir_all(&managed, Some(&mut m)).unwrap();
        std::fs::write(join_path(&[&managed, "leftover.txt"]), b"a").unwrap();
        // Not a direct child of skills_parent → untouched.
        let elsewhere = join_path(&[&root, "other", "thing"]);
        record_mkdir_all(&elsewhere, Some(&mut m)).unwrap();
        write_sidecar_manifest(&env_root, &m).unwrap();

        remove_reused_managed_skill_dirs(&env_root, &skills_parent).unwrap();
        assert!(!Path::new(&managed).exists());
        assert!(Path::new(&elsewhere).exists());
    }

    // Port of TestDirOf (filepath.Dir semantics).
    #[test]
    fn test_dir_of() {
        assert_eq!(dir_of("/a/b/c"), "/a/b");
        assert_eq!(dir_of("/a"), "/");
        assert_eq!(dir_of("a"), ".");
        assert_eq!(dir_of("a/b"), "a");
        assert_eq!(dir_of(""), ".");
    }
}
