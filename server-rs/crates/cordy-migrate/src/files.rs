//! Migration file discovery — port of `server/internal/migrations/migrations.go`.

use std::path::{Path, PathBuf};

const MAX_SEARCH_DEPTH: usize = 4;
const CANDIDATE_LEAVES: [&str; 2] = ["migrations", "server/migrations"];

/// Find the migrations directory by walking up from the current directory
/// (then the executable's directory), matching the Go runner's resolution.
pub fn resolve_dir() -> anyhow::Result<PathBuf> {
    let mut roots = vec![std::env::current_dir()?];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
        }
    }

    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let mut base = root;
        for _ in 0..=MAX_SEARCH_DEPTH {
            for leaf in CANDIDATE_LEAVES {
                let dir = base.join(leaf);
                if seen.insert(dir.clone()) && dir.is_dir() {
                    return Ok(dir);
                }
            }
            base = match base.parent() {
                Some(p) => p.to_path_buf(),
                None => break,
            };
        }
    }
    anyhow::bail!("migrations directory not found")
}

/// Sorted migration files for `direction` ("up" or "down").
/// Up runs oldest-first; down runs newest-first.
pub fn files(direction: &str) -> anyhow::Result<Vec<PathBuf>> {
    let dir = resolve_dir()?;
    let suffix = format!(".{direction}.sql");

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&suffix))
        })
        .collect();

    if direction == "down" {
        files.sort_unstable_by(|a, b| b.cmp(a));
    } else {
        files.sort_unstable();
    }
    Ok(files)
}

/// Every "up" version in apply order. Readiness must verify ALL of them are
/// recorded — an out-of-order migration below an applied later one would
/// otherwise be missed.
pub fn all_versions() -> anyhow::Result<Vec<String>> {
    let files = files("up")?;
    if files.is_empty() {
        anyhow::bail!("no up migrations found");
    }
    Ok(files.iter().map(|f| extract_version(f)).collect())
}

/// Strip the `.up.sql` / `.down.sql` suffix from a migration file name.
pub fn extract_version(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    name.strip_suffix(".up.sql")
        .or_else(|| name.strip_suffix(".down.sql"))
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_version_strips_direction_suffix() {
        assert_eq!(extract_version(Path::new("001_init.up.sql")), "001_init");
        assert_eq!(
            extract_version(Path::new("007_drop_issue_repository.down.sql")),
            "007_drop_issue_repository"
        );
    }

    #[test]
    fn resolve_dir_finds_repo_migrations_from_workspace() {
        // When run via `cargo test` inside server-rs/, the repo root is 1-2
        // levels up and contains server/migrations/.
        let dir = resolve_dir().expect("migrations dir should be discoverable");
        assert!(dir.join("001_init.up.sql").exists());
    }
}
