//! Port of `server/internal/daemon/execenv/codex_user_skills.go` (106 lines).
//!
//! Symbol map:
//! - seedUserCodexSkills → seed_user_codex_skills
//! - createDirLink (codex_home_link.go, unix build tag) → create_dir_link
//!
//! Deviations from Go: slog → tracing with identical message text;
//! filepath.EvalSymlinks → std::fs::canonicalize.

use std::collections::HashSet;

use super::context::sanitize_skill_name;
use super::execenv::{join_path, user_home_dir, SkillContextForEnv};

/// createDirLink (codex_home_link.go, !windows): plain symlink.
fn create_dir_link(src: &str, dst: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(src, dst)
    }
}

/// resolveSharedCodexHome stand-in: the shared ~/.codex home. The full port
/// lives in codex_home.go (lane E1b); seed only needs the directory.
fn shared_codex_home() -> anyhow::Result<String> {
    Ok(join_path(&[&user_home_dir()?, ".codex"]))
}

/// seedUserCodexSkills links user-installed skill directories from the shared
/// ~/.codex/skills/ into the per-task CODEX_HOME so the codex CLI discovers
/// them natively. Codex is the only runtime whose HOME is redirected to a
/// per-task directory (via the CODEX_HOME env var), so without this step the
/// CLI never sees the user's `~/.codex/skills/` content.
///
/// Links, never copies — see codex_user_skills.go for the full rationale.
///
/// Workspace-assigned skills take precedence on name conflict; per-skill
/// failures are logged and skipped.
pub(crate) fn seed_user_codex_skills(
    codex_home: &str,
    workspace_skills: &[SkillContextForEnv],
) -> anyhow::Result<()> {
    let shared_skills_dir = join_path(&[&shared_codex_home()?, "skills"]);

    let info = match std::fs::metadata(&shared_skills_dir) {
        Ok(info) => info,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("stat shared skills dir {shared_skills_dir}")))
        }
    };
    if !info.is_dir() {
        return Ok(());
    }

    let reserved: HashSet<String> = workspace_skills
        .iter()
        .map(|s| sanitize_skill_name(&s.name))
        .collect();

    let entries = std::fs::read_dir(&shared_skills_dir)
        .map_err(|e| anyhow::Error::new(e).context("read shared skills dir"))?;

    let target_skills_dir = join_path(&[codex_home, "skills"]);
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    // os.ReadDir returns lexical order; read_dir is platform-dependent.
    names.sort();

    for name in names {
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        if reserved.contains(&sanitize_skill_name(&name)) {
            tracing::info!(name = %name, "execenv: codex user-skill yields to workspace skill");
            continue;
        }
        let src = join_path(&[&shared_skills_dir, &name]);
        // Installers like lark-cli ship each skill as a symlink into a
        // shared ~/.agents/skills/<name>/ directory. Resolve it so the
        // per-task link points at the real directory instead of at another
        // link, and so the is_dir check below sees the actual target.
        let resolved = match std::fs::canonicalize(&src) {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(name = %name, error = %err, "execenv: codex user-skill resolve failed");
                continue;
            }
        };
        let fi = match std::fs::metadata(&resolved) {
            Ok(fi) => fi,
            Err(_) => continue,
        };
        if !fi.is_dir() {
            continue;
        }
        // hydrateCodexSkills wipes the skills dir before every seed, so the
        // parent is normally absent here; created lazily so a task with no
        // eligible user skills still gets no skills directory at all.
        std::fs::create_dir_all(&target_skills_dir).map_err(|e| {
            anyhow::Error::new(e).context(format!("create codex skills dir {target_skills_dir}"))
        })?;
        let dst = join_path(&[&target_skills_dir, &name]);
        // Removes the link, never the link target: remove_dir_all unlinks a
        // symlink on unix, and Windows RemoveDirectory drops a junction while
        // leaving the directory it points at intact.
        if let Err(err) = match std::fs::remove_dir_all(&dst) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => std::fs::remove_file(&dst),
        } {
            tracing::warn!(name = %name, error = %err, "execenv: codex user-skill clean dst failed");
            continue;
        }
        let resolved_str = resolved.to_string_lossy().into_owned();
        if let Err(err) = create_dir_link(&resolved_str, &dst) {
            tracing::warn!(name = %name, error = %err, "execenv: codex user-skill link failed");
            continue;
        }
    }
    Ok(())
}
