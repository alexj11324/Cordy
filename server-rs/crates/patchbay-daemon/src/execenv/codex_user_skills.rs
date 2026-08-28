//! Port of execenv/codex_user_skills.go.
//!
//! Symbol map:
//! - seedUserCodexSkills → seed_user_codex_skills
//!
//! Deviations:
//! - slog logger parameters dropped; tracing macros used directly.
//! - filepath.EvalSymlinks → std::fs::canonicalize.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use super::codex_home::resolve_shared_codex_home;
use super::execenv::{join_path, SkillContextForEnv};

/// seed_user_codex_skills links user-installed skill directories from the
/// shared ~/.codex/skills/ into the per-task CODEX_HOME so the codex CLI
/// discovers them natively.
///
/// Links, never copies. A copy charges the whole skill tree to every task
/// directory — 100 MB for a user with a handful of npm-backed skills — and
/// hydrateCodexSkills re-does it on every task start, on the critical path,
/// while no GC path ever reclaims it as long as the issue stays open. The two
/// other shared resources in the same per-task home already link for the same
/// reason: sessions (linkCodexSessionsToStore) and the plugin cache
/// (exposeSharedCodexPluginCache). A skill directory is read-only input to the
/// CLI, so the write isolation a copy incidentally bought has no requester.
///
/// Workspace-assigned skills take precedence on name conflict: any user skill
/// whose sanitized name is reserved by a workspace skill is skipped.
///
/// allocateCollisionFreeSkillDir probes with os.Lstat, so a link left here
/// counts as occupied and the workspace skill lands in a `-patchbay` sibling.
///
/// Per-skill failures are logged and skipped — a single broken user skill must
/// not prevent the task from running. Returning an error is reserved for
/// failures that would affect every skill: listing the shared skills directory,
/// or creating the per-task skills directory.
pub(crate) fn seed_user_codex_skills(
    codex_home: &str,
    workspace_skills: &[SkillContextForEnv],
) -> Result<()> {
    let shared_skills_dir = join_path(&[&resolve_shared_codex_home(), "skills"]);

    let info = match std::fs::metadata(&shared_skills_dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(anyhow::anyhow!("stat shared skills dir: {e}")),
        Ok(i) => i,
    };
    if !info.is_dir() {
        return Ok(());
    }

    let reserved: HashSet<String> = workspace_skills
        .iter()
        .map(|s| super::context::sanitize_skill_name(&s.name))
        .collect();

    let entries = std::fs::read_dir(&shared_skills_dir)
        .with_context(|| format!("read shared skills dir {shared_skills_dir}"))?;

    let target_skills_dir = join_path(&[codex_home, "skills"]);
    let mut created_target = false;
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        if reserved.contains(&super::context::sanitize_skill_name(&name)) {
            tracing::info!(name = %name, "execenv: codex user-skill yields to workspace skill");
            continue;
        }
        let src = join_path(&[&shared_skills_dir, &name]);
        // Installers like lark-cli ship each skill as a symlink into a shared
        // ~/.agents/skills/<name>/ directory. Resolve it so the per-task link
        // points at the real directory instead of at another link.
        let resolved = match std::fs::canonicalize(&src) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(err) => {
                tracing::warn!(name = %name, error = %err, "execenv: codex user-skill resolve failed");
                continue;
            }
        };
        match std::fs::metadata(&resolved) {
            Ok(md) if md.is_dir() => {}
            _ => continue,
        }
        // hydrateCodexSkills wipes the skills dir before every seed, so the
        // parent is normally absent here; createDirLink needs it to exist.
        // Created lazily so a task with no eligible user skills still gets no
        // skills directory at all.
        if !created_target {
            std::fs::create_dir_all(&target_skills_dir)
                .with_context(|| format!("create codex skills dir {target_skills_dir}"))?;
            created_target = true;
        }
        let dst = join_path(&[&target_skills_dir, &name]);
        // Removes the link, never the link target (remove_any unlinks a symlink).
        if let Err(err) = remove_link(&dst) {
            tracing::warn!(name = %name, error = %format!("{err:#}"), "execenv: codex user-skill clean dst failed");
            continue;
        }
        if let Err(err) = super::codex_home::create_dir_link(&resolved, &dst) {
            tracing::warn!(name = %name, error = %format!("{err:#}"), "execenv: codex user-skill link failed");
            continue;
        }
    }
    Ok(())
}

fn remove_link(path: &str) -> anyhow::Result<()> {
    match Path::new(path).symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => std::fs::remove_file(path)
            .or_else(|_| std::fs::remove_dir(path))
            .map_err(Into::into),
    }
}
