//! Port of execenv/git.go.
//!
//! Symbol map:
//! - detectGitRepo          → detect_git_repo
//! - fetchOrigin            → fetch_origin
//! - getRemoteDefaultBranch → get_remote_default_branch
//! - setupGitWorktree       → setup_git_worktree
//! - runGitWorktreeAdd      → run_git_worktree_add
//! - removeGitWorktree      → remove_git_worktree
//! - excludeFromGit         → exclude_from_git
//! - repoNameFromURL        → repo_name_from_url
//! - taskKeyLen / taskKey   → TASK_KEY_LEN / task_key
//! - shortID                → short_id
//! - sanitizeName           → sanitize_name
//!
//! Deviations: git subprocesses run through tokio::process (async) instead of
//! os/exec; Go's `*exec.ExitError` text ("exit status N") becomes an anyhow
//! error carrying the same wrapped context strings. git.go itself sets no
//! timeout on these invocations and neither do we (local_worktree.rs keeps its
//! own two-minute bound).

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use regex::Regex;
use tokio::process::Command;
use tracing::warn;

/// DetectGitRepo checks if dir is inside a git repository (regular or bare).
/// Returns the git root path and true if found.
pub async fn detect_git_repo(dir: &str) -> Option<String> {
    // Try regular repo first.
    let out = Command::new("git")
        .args(["-C", dir, "rev-parse", "--show-toplevel"])
        .output()
        .await
        .ok()?;
    if out.status.success() {
        return Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }

    // Try bare repo: git-dir is "." for bare repos when -C points at the repo.
    let out = Command::new("git")
        .args(["-C", dir, "rev-parse", "--is-bare-repository"])
        .output()
        .await
        .ok()?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true" {
        return Some(dir.to_string());
    }

    None
}

/// FetchOrigin runs `git fetch origin` to ensure the local repo has the latest remote refs.
pub async fn fetch_origin(git_root: &str) -> anyhow::Result<()> {
    let out = Command::new("git")
        .args(["-C", git_root, "fetch", "origin"])
        .output()
        .await
        .context("run git fetch origin")?;
    if !out.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        bail!("git fetch origin: {}: exit status {}", combined.trim(), out.status);
    }
    Ok(())
}

/// GetRemoteDefaultBranch returns "origin/<branch>" for the remote's default branch.
/// Falls back to "origin/main", then "HEAD".
pub async fn get_remote_default_branch(git_root: &str) -> String {
    // Try symbolic-ref of origin/HEAD (set by `git clone` or `git remote set-head`).
    if let Ok(out) = Command::new("git")
        .args(["-C", git_root, "symbolic-ref", "refs/remotes/origin/HEAD"])
        .output()
        .await
    {
        if out.status.success() {
            let ref_name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // ref looks like "refs/remotes/origin/main" — return "origin/main".
            if let Some(stripped) = ref_name.strip_prefix("refs/remotes/") {
                return stripped.to_string();
            }
            return ref_name;
        }
    }

    // Fallback: check if origin/main exists.
    if let Ok(out) = Command::new("git")
        .args(["-C", git_root, "rev-parse", "--verify", "origin/main"])
        .output()
        .await
    {
        if out.status.success() {
            return "origin/main".to_string();
        }
    }

    // Fallback: check if origin/master exists.
    if let Ok(out) = Command::new("git")
        .args(["-C", git_root, "rev-parse", "--verify", "origin/master"])
        .output()
        .await
    {
        if out.status.success() {
            return "origin/master".to_string();
        }
    }

    "HEAD".to_string()
}

/// SetupGitWorktree creates a git worktree at worktreePath with a new branch.
pub async fn setup_git_worktree(
    git_root: &str,
    worktree_path: &str,
    branch_name: &str,
    base_ref: &str,
) -> anyhow::Result<()> {
    // Remove the workdir created by caller — git worktree add needs to create it.
    match std::fs::remove_dir(worktree_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            // A placeholder workdir could be a file in pathological setups; Go's
            // os.Remove removes both. Fall back to remove_file before failing.
            std::fs::remove_file(worktree_path)
                .with_context(|| format!("remove placeholder workdir {worktree_path}"))?;
            let _ = e;
        }
    }

    match run_git_worktree_add(git_root, worktree_path, branch_name, base_ref).await {
        Ok(()) => Ok(()),
        Err(err) => {
            let msg = format!("{err:#}");
            if msg.contains("already exists") {
                // Branch name collision: append timestamp and retry once.
                // Go reassigns branchName locally but returns only the error —
                // the timestamped name is never reported back to callers.
                let branch_name =
                    format!("{branch_name}-{}", chrono::Utc::now().timestamp());
                run_git_worktree_add(git_root, worktree_path, &branch_name, base_ref).await
            } else {
                Err(err)
            }
        }
    }
}

async fn run_git_worktree_add(
    git_root: &str,
    worktree_path: &str,
    branch_name: &str,
    base_ref: &str,
) -> anyhow::Result<()> {
    let out = Command::new("git")
        .args([
            "-C",
            git_root,
            "worktree",
            "add",
            "-b",
            branch_name,
            worktree_path,
            base_ref,
        ])
        .output()
        .await
        .context("run git worktree add")?;
    if !out.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        bail!(
            "git worktree add: {}: exit status {}",
            combined.trim(),
            out.status
        );
    }
    Ok(())
}

/// RemoveGitWorktree removes a worktree and its branch. Best-effort: logs errors.
pub async fn remove_git_worktree(
    git_root: &str,
    worktree_path: &str,
    branch_name: &str,
) {
    // Remove the worktree.
    if let Ok(out) = Command::new("git")
        .args(["-C", git_root, "worktree", "remove", "--force", worktree_path])
        .output()
        .await
    {
        if !out.status.success() {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            warn!(
                output = %combined.trim(),
                error = %out.status,
                "execenv: git worktree remove failed"
            );
        }
    }

    // Delete the branch (best-effort).
    if !branch_name.is_empty() {
        if let Ok(out) = Command::new("git")
            .args(["-C", git_root, "branch", "-D", branch_name])
            .output()
            .await
        {
            if !out.status.success() {
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                warn!(
                    branch = %branch_name,
                    output = %combined.trim(),
                    error = %out.status,
                    "execenv: git branch delete failed"
                );
            }
        }
    }
}

/// ExcludeFromGit adds a pattern to the worktree's .git/info/exclude file.
pub fn exclude_from_git(worktree_path: &str, pattern: &str) -> anyhow::Result<()> {
    // Resolve the actual git dir for this worktree. Kept synchronous like the
    // rest of the exclude bookkeeping (a single fast rev-parse).
    let out = std::process::Command::new("git")
        .args(["-C", worktree_path, "rev-parse", "--git-dir"])
        .output()
        .context("resolve git dir")?;
    if !out.status.success() {
        bail!("resolve git dir: exit status {}", out.status);
    }

    let mut git_dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let git_dir_path = Path::new(&git_dir);
    if !git_dir_path.is_absolute() {
        git_dir = join_path(&[worktree_path, &git_dir]);
    }

    let exclude_path = PathBuf::from(&git_dir).join("info").join("exclude");

    // Ensure the info directory exists.
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent).context("create info dir")?;
    }

    // Check if pattern is already present.
    if let Ok(existing) = std::fs::read_to_string(&exclude_path) {
        if existing.contains(pattern) {
            return Ok(());
        }
    }

    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&exclude_path)
        .context("open exclude file")?;
    writeln!(f, "\n{pattern}").context("write exclude pattern")?;
    Ok(())
}

/// RepoNameFromURL extracts a short directory name from a git remote URL.
/// e.g. "https://github.com/org/my-repo.git" → "my-repo"
pub fn repo_name_from_url(url: &str) -> String {
    // Strip trailing slashes and .git suffix.
    let mut url = url.trim_end_matches('/').to_string();
    if let Some(stripped) = url.strip_suffix(".git") {
        url = stripped.to_string();
    }

    // Take the last path segment.
    if let Some(i) = url.rfind('/') {
        url = url[i + 1..].to_string();
    }
    // Also handle SSH-style "host:org/repo".
    if let Some(i) = url.rfind(':') {
        url = url[i + 1..].to_string();
        if let Some(j) = url.rfind('/') {
            url = url[j + 1..].to_string();
        }
    }

    let name = url.trim();
    if name.is_empty() {
        return "repo".to_string();
    }
    name.to_string()
}

/// TaskKeyLen is how many hex chars of the task id identify a task in a path or
/// a branch name. Every char here is spent twice — the env root prefixes the
/// agent's whole workdir, and the branch name becomes a path under
/// .git/refs/heads/ inside that workdir — and Windows still enforces MAX_PATH
/// (260). The full 32-char id overflows it on a deep checkout, so the segment
/// stays short and buys its uniqueness from entropy instead of length.
const TASK_KEY_LEN: usize = 12;

/// TaskKey returns the segment identifying a task in a path or branch name: the
/// LAST taskKeyLen hex chars of the id.
///
/// Which end matters more than how many chars. Task ids are UUIDv7 — 48 bits of
/// millisecond timestamp, then randomness. The leading 8 hex chars are the high
/// 32 bits of that timestamp, so they only advance once every 2^16 ms (~65.5s):
/// taking them from the front gave every task started inside one such window an
/// identical segment, and therefore one shared env root. That is not a rare hash
/// collision, it is the common case, and it made Prepare's "remove existing env"
/// step delete a concurrently running task's directory (#7326).
///
/// The tail is drawn from the id's random field, so 12 chars carry 48 random
/// bits. Prepare additionally refuses to delete an env root another task owns,
/// so even an improbable clash fails closed instead of destroying work.
///
/// Use shortID for logs, never for identity.
pub fn task_key(uuid: &str) -> String {
    let s = uuid.replace('-', "");
    if s.len() > TASK_KEY_LEN {
        return s[s.len() - TASK_KEY_LEN..].to_string();
    }
    s
}

/// ShortID returns the first 8 characters of a UUID string (dashes stripped).
/// Display and logging only — see task_key for anything that must be unique.
pub fn short_id(uuid: &str) -> String {
    let s = uuid.replace('-', "");
    if s.len() > 8 {
        return s[..8].to_string();
    }
    s
}

fn non_alphanumeric_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^a-z0-9]+").expect("static regex"))
}

/// SanitizeName produces a git-branch-safe name from a human-readable string.
pub fn sanitize_name(name: &str) -> String {
    let s = name.trim().to_lowercase();
    let s = non_alphanumeric_re().replace_all(&s, "-");
    let mut s = s.trim_matches('-').to_string();
    if s.len() > 30 {
        s.truncate(30);
        let trimmed = s.trim_end_matches('-').to_string();
        s = trimmed;
    }
    if s.is_empty() {
        return "agent".to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestShortID.
    #[test]
    fn test_short_id() {
        let cases = [
            ("a1b2c3d4-e5f6-7890-abcd-ef1234567890", "a1b2c3d4"),
            ("abcdef12", "abcdef12"),
            ("ab", "ab"),
            ("a1b2c3d4e5f67890", "a1b2c3d4"),
        ];
        for (input, want) in cases {
            assert_eq!(short_id(input), want, "shortID({input:?})");
        }
    }

    // Port of TestSanitizeName.
    #[test]
    fn test_sanitize_name() {
        let cases = [
            ("Code Reviewer", "code-reviewer"),
            ("my_agent!@#v2", "my-agent-v2"),
            ("  spaces  ", "spaces"),
            ("UPPERCASE", "uppercase"),
            (
                "a-very-long-name-that-exceeds-thirty-characters-total",
                "a-very-long-name-that-exceeds",
            ),
            ("", "agent"),
            ("---", "agent"),
            ("日本語テスト", "agent"),
        ];
        for (input, want) in cases {
            assert_eq!(sanitize_name(input), want, "sanitizeName({input:?})");
        }
    }

    // Port of TestRepoNameFromURL.
    #[test]
    fn test_repo_name_from_url() {
        let cases = [
            ("https://github.com/org/my-repo.git", "my-repo"),
            ("https://github.com/org/my-repo", "my-repo"),
            ("git@github.com:org/my-repo.git", "my-repo"),
            ("https://github.com/org/repo/", "repo"),
            ("my-repo", "my-repo"),
            ("", "repo"),
        ];
        for (input, want) in cases {
            assert_eq!(repo_name_from_url(input), want, "repoNameFromURL({input:?})");
        }
    }

    // Port of TestTaskKeyReadsTheRandomTail.
    #[test]
    fn test_task_key_reads_the_random_tail() {
        const ID: &str = "01a01ec0-e69d-7000-8000-0123456789ab";
        let got = task_key(ID);
        assert_eq!(
            got.len(),
            TASK_KEY_LEN,
            "taskKey({ID}) = {got} — long segments overflow MAX_PATH on Windows"
        );
        assert_eq!(
            got, "0123456789ab",
            "the segment must come from the random tail, not the timestamp head"
        );
        // Two ids sharing a UUIDv7 timestamp head still get distinct keys.
        let other = "01a01ec0-e69d-7000-8000-fedcba987654";
        assert_ne!(task_key(other), got);
        // Sub-length inputs pass through unchanged.
        assert_eq!(task_key("abc"), "abc");
    }
}
