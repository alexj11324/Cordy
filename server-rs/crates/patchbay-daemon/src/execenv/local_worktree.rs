//! Port of execenv/local_worktree.go.
//!
//! Symbol map:
//! - LocalWorktreeDirName            → LOCAL_WORKTREE_DIR_NAME
//! - gitTimeout / maxUntrackedFiles /
//!   maxUntrackedBytes               → GIT_TIMEOUT / MAX_UNTRACKED_FILES / MAX_UNTRACKED_BYTES
//! - gitRootLocks / lockGitRoot      → git_root_locks / lock_git_root
//! - LocalWorktreeParams             → LocalWorktreeParams
//! - LocalWorktree                   → LocalWorktree
//! - LocalWorktreeOutcome            → LocalWorktreeOutcome
//! - PrepareLocalWorktree            → prepare_local_worktree
//! - Finalize                        → LocalWorktree::finalize
//! - Discard                         → LocalWorktree::discard
//! - AbortWithReason                 → LocalWorktree::abort_with_reason
//! - commitBaseline                  → commit_baseline
//! - commitAll                       → LocalWorktree::commit_all
//! - commitEverything                → commit_everything
//! - commitIdentityArgs              → commit_identity_args
//! - worktreeIsDirty                 → worktree_is_dirty
//! - removeLocalWorktreeDir          → remove_local_worktree_dir
//! - deleteBranch                    → delete_branch
//! - resolveGitRoot                  → resolve_git_root
//! - addLocalWorktree                → add_local_worktree
//! - copyUntrackedFiles              → copy_untracked_files
//! - patchbaySidecarDirNames /
//!   isPatchbaySidecarPath              → PATCHBAY_SIDECAR_DIR_NAMES / is_patchbay_sidecar_path
//! - copyUntrackedFile               → copy_untracked_file
//! - runGit / runGitTrimmed /
//!   runGitStdout                    → run_git / run_git_trimmed / run_git_stdout
//!
//! Deviations:
//! - slog logger parameters dropped; tracing macros used directly.
//! - The per-repo mutex is a tokio::sync::Mutex map held in async fns; Go's
//!   sync.Map of std Mutexes becomes a std::sync::Mutex<HashMap> registry.
//! - aborted error is stored as its rendered chain text (`format!("{err:#}")`)
//!   since anyhow::Error is not Clone and the value is only ever displayed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail};
use tokio::process::Command;

use super::execenv::join_path;
use super::git::{sanitize_name, task_key};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// localWorktreeDirName: env-root-relative directory holding the worktree.
/// Kept short: on Windows the worktree path plus the deepest repo path must
/// stay under MAX_PATH for tools that predate long paths.
const LOCAL_WORKTREE_DIR_NAME: &str = "worktree";

/// gitTimeout bounds every git invocation this file makes. These are all
/// local-only operations (no network), so a slow one means a wedged index
/// lock rather than a slow remote; failing the task beats hanging a daemon
/// slot forever.
const GIT_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// maxUntrackedFiles / maxUntrackedBytes bound the untracked-file replay.
/// `--exclude-standard` already drops anything gitignored (node_modules,
/// build output, venvs), so a repo hitting these limits has an unusual
/// amount of untracked-but-not-ignored content. We copy up to the bound and
/// report the remainder rather than silently truncating or hanging on a
/// multi-gigabyte copy.
const MAX_UNTRACKED_FILES: usize = 2000;
const MAX_UNTRACKED_BYTES: i64 = 200 << 20; // 200 MiB

// ---------------------------------------------------------------------------
// Per-repo locks
// ---------------------------------------------------------------------------

/// gitRootLocks serialises git admin operations per repository. Concurrent
/// `git worktree add` / `remove` / `prune` on one repo race on the same
/// lockfiles (worktrees/, packed-refs.lock, config.lock), and unlike a fetch
/// these are fast, so a plain mutex costs nothing. Keyed by the repo root so
/// tasks on different repos never wait on each other.
fn git_root_locks() -> &'static std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

type GitRootGuard = tokio::sync::OwnedMutexGuard<()>;

/// Locks the per-repo mutex and returns the guard; callers hold it for the
/// duration of their git admin operations (Go's `defer unlock()`).
async fn lock_git_root(git_root: &str) -> (Arc<tokio::sync::Mutex<()>>, GitRootGuard) {
    let key = {
        let mut map = git_root_locks().lock().unwrap();
        map.entry(git_root.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let guard = key.clone().lock_owned().await;
    (key, guard)
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// LocalWorktreeParams describes the worktree Prepare should build for a
/// local_directory task running in worktree mode.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LocalWorktreeParams {
    /// LocalPath is the user's configured directory. It may be the repo root
    /// or any subdirectory of it; the worktree always covers the whole repo,
    /// and the agent's cwd is the matching subdirectory inside it.
    pub local_path: String,
    /// EnvRoot is the daemon-owned task env root. The worktree is created
    /// inside it so the ordinary env-root GC reclaims it.
    pub env_root: String,
    /// AgentName and TaskID name the branch: agent/<name>/<short-task-id>.
    pub agent_name: String,
    pub task_id: String,
}

/// LocalWorktree is a prepared worktree plus everything the daemon needs to
/// finalize it after the agent exits.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LocalWorktree {
    /// GitRoot is the user's repository root — the repo that owns the branch.
    #[serde(rename = "GitRoot")]
    pub git_root: String,
    /// Path is the worktree root inside the env root.
    #[serde(rename = "Path")]
    pub path: String,
    /// WorkDir is the agent's cwd: Path, plus the offset of LocalPath inside
    /// the repo when the user pointed the resource at a subdirectory.
    #[serde(rename = "WorkDir")]
    pub work_dir: String,
    /// Branch is the branch created for this task, in the user's repo.
    #[serde(rename = "Branch")]
    pub branch: String,
    /// RepoIdentity is the canonical remote repository identity captured when
    /// the task worktree is prepared. It is used to scope terminal PR
    /// discovery; it is never inferred from a PR title, body, or branch name.
    #[serde(rename = "RepoIdentity")]
    pub repo_identity: String,
    /// BaseCommit is the commit the worktree started from. Finalize compares
    /// the branch tip against it to decide whether the task produced anything.
    #[serde(rename = "BaseCommit")]
    pub base_commit: String,
    /// DirtyBaseCaptured records that the user had uncommitted tracked edits
    /// which were replayed into the worktree.
    #[serde(rename = "DirtyBaseCaptured")]
    pub dirty_base_captured: bool,
    // aborted is deliberately NOT serialized: it is in-process state only.
    /// aborted, when set, makes Finalize refuse to commit or remove anything.
    /// Set by the daemon when a pre-commit step failed in a way that would make
    /// the committed branch wrong (see AbortWithReason).
    #[serde(skip)]
    pub(crate) aborted: Option<String>,
    /// UntrackedCopied / UntrackedSkipped report the untracked-file replay.
    /// A non-zero skip count means the bounds below were hit and the agent is
    /// looking at less than the user has on disk; it is logged at warn level so
    /// the gap is findable rather than invisible.
    #[serde(rename = "UntrackedCopied")]
    pub untracked_copied: i32,
    #[serde(rename = "UntrackedSkipped")]
    pub untracked_skipped: i32,
}

/// LocalWorktreeOutcome is what a finished worktree task delivered.
#[derive(Debug, Clone, Default)]
pub struct LocalWorktreeOutcome {
    /// Branch is the branch holding the task's work, or "" when the task made
    /// no changes at all (a read-only run) — in that case the branch is deleted
    /// so it never shows up in the user's `git branch` as an empty artifact.
    pub branch: String,
    /// AutoCommitted is true when the agent left uncommitted changes that
    /// Finalize committed so they would survive the worktree's removal.
    pub auto_committed: bool,
    /// PreservedPath is set only when Finalize could NOT commit the agent's
    /// changes. The worktree at this path was intentionally left on disk because
    /// it is the only remaining copy of that work.
    pub preserved_path: String,
    /// Provenance needed by the server's terminal exact-head discovery.
    pub repo_identity: String,
    pub execution_workspace: String,
    pub head_sha: String,
    pub head_state: String,
}

// ---------------------------------------------------------------------------
// Prepare
// ---------------------------------------------------------------------------

/// PrepareLocalWorktree creates the task's worktree and replays the user's
/// uncommitted state into it. It never writes to the user's working tree: the
/// dirty state is read through `git stash create`, which builds a commit object
/// without touching the index or the files on disk.
pub async fn prepare_local_worktree(params: LocalWorktreeParams) -> anyhow::Result<LocalWorktree> {
    if params.local_path.is_empty() {
        bail!("execenv: local worktree requires a local path");
    }
    if params.env_root.is_empty() {
        bail!("execenv: local worktree requires an env root");
    }
    if params.task_id.is_empty() {
        bail!("execenv: local worktree requires a task id");
    }

    let git_root = resolve_git_root(&params.local_path).await?;
    let repo_identity = run_git_trimmed(&git_root, ["remote", "get-url", "origin"])
        .await
        .unwrap_or_default();

    // The agent's cwd keeps the user's chosen depth: a resource pointed at
    // <repo>/services/api must land the agent in <worktree>/services/api, not
    // at the repo root, or the task's whole notion of "the project" shifts.
    //
    // Canonicalise before the comparison: gitRoot comes back canonical, while
    // the configured path routinely isn't (on macOS every /tmp and /var path is
    // a symlink into /private). Comparing the two forms directly reads a repo
    // root as "outside itself".
    let mut local_path = params.local_path.clone();
    if let Ok(resolved) = std::fs::canonicalize(&local_path) {
        local_path = resolved.to_string_lossy().into_owned();
    }
    let rel = match path_rel(&git_root, &local_path) {
        Some(rel) => rel,
        None => bail!(
            "execenv: locate {:?} inside repo {:?}: not a relative path",
            local_path,
            git_root
        ),
    };
    if rel == ".." || rel.starts_with("../") {
        bail!(
            "execenv: {:?} is not inside its repository root {:?}",
            local_path,
            git_root
        );
    }

    let worktree_path = join_path(&[&params.env_root, LOCAL_WORKTREE_DIR_NAME]);

    // Everything below mutates the repo's worktree admin state, so take the
    // per-repo lock first — including the stale-path cleanup, which runs `git
    // worktree remove` and would otherwise race a sibling task's `worktree add`.
    let (_lock, _guard) = lock_git_root(&git_root).await;

    if Path::new(&worktree_path).symlink_metadata().is_ok() {
        // Prepare wipes and recreates envRoot, so an existing worktree path
        // means a stale registration in the user's repo pointing here. Remove
        // both rather than failing the task.
        let _ = remove_local_worktree_dir(&git_root, &worktree_path).await;
    }

    // Self-heal registrations orphaned by a crashed daemon: their env roots are
    // long gone, but the user's repo still lists them. Prune only drops entries
    // whose directory no longer exists, so it can never disturb a live task.
    if let Err((out, err)) = run_git(&git_root, ["worktree", "prune"]).await {
        tracing::warn!(
            git_root = %git_root,
            output = %out.trim(),
            error = %format!("{err:#}"),
            "execenv: git worktree prune failed (non-fatal)"
        );
    }

    let head_sha = run_git_trimmed(&git_root, ["rev-parse", "--verify", "HEAD"])
        .await
        .map_err(|e| {
            anyhow!(
                "execenv: repository {:?} has no commit to branch from \
                 (worktree mode needs at least one commit; make an initial commit or switch the resource back to in_place): {e:#}",
                git_root
            )
        })?;

    // `git stash create` builds a commit capturing tracked modifications and
    // returns its sha WITHOUT stashing — the user's index and working tree are
    // untouched. Empty output means the tree is clean. The identity args cover
    // a repo with no user.email configured: writing a commit object needs a
    // committer, and without them the user's uncommitted work would be dropped
    // on a technicality.
    let identity = commit_identity_args(&git_root).await;
    let mut stash_args: Vec<&str> = identity.iter().map(String::as_str).collect();
    stash_args.extend(["stash", "create"]);
    let stash_sha = match run_git_trimmed(&git_root, stash_args).await {
        Ok(sha) => sha,
        // Fail closed. The promise of this mode is that the agent reasons about
        // the code the user actually has; silently starting from HEAD instead
        // would have it review a tree the user never saw and report confidently
        // on it. A task that does not start is recoverable — one that answers
        // from the wrong sources is not.
        Err(e) => bail!(
            "execenv: could not capture the uncommitted changes in {:?}, \
             so the worktree would not match what you have on disk: {e:#}",
            git_root
        ),
    };

    let branch = format!(
        "agent/{}/{}",
        sanitize_name(&params.agent_name),
        task_key(&params.task_id)
    );
    let actual_branch = add_local_worktree(&git_root, &worktree_path, &branch, &head_sha).await?;

    let mut wt = LocalWorktree {
        git_root: git_root.clone(),
        path: worktree_path.clone(),
        work_dir: join_path(&[&worktree_path, &rel]),
        branch: actual_branch.clone(),
        repo_identity,
        base_commit: head_sha,
        ..Default::default()
    };

    // Replay tracked edits. Applied as unstaged modifications on top of HEAD so
    // the branch history stays linear and the agent sees the same
    // work-in-progress the user has open in their editor.
    //
    // Every failure below aborts the prepare and tears the worktree back down.
    // A half-replayed tree is the worst outcome available: it looks like a
    // working checkout, so nothing downstream questions it, while the agent
    // silently reads different code than the user has.
    if !stash_sha.is_empty() {
        if let Err((out, apply_err)) = run_git(&worktree_path, ["stash", "apply", &stash_sha]).await
        {
            let _ = remove_local_worktree_dir(&git_root, &worktree_path).await;
            let _ = delete_branch(&git_root, &actual_branch).await;
            bail!(
                "execenv: could not replay the uncommitted changes from {:?} into the task worktree \
                 (the agent would have seen a different tree than you have): {}: {apply_err:#}",
                git_root,
                out.trim()
            );
        }
        wt.dirty_base_captured = true;
    }

    let (copied, skipped) = copy_untracked_files(&git_root, &worktree_path).await;
    if skipped > 0 {
        // Any untracked file we could not reproduce makes the worktree a tree
        // the user would not recognise, so this fails rather than quietly
        // under-copying. Causes: the size/count bounds (usually build output
        // that should have been gitignored), an untracked symlink, or a file
        // that disappeared mid-snapshot. The message names the common fix
        // without claiming to know which one it was.
        let _ = remove_local_worktree_dir(&git_root, &worktree_path).await;
        let _ = delete_branch(&git_root, &actual_branch).await;
        bail!(
            "execenv: could not replay every untracked file from {:?} into the task worktree \
             (copied {}, {} left over; the replay covers regular files up to {} files / {} MiB and does not follow symlinks) \
             — gitignore or clean up the untracked files, or switch the resource back to in_place",
            git_root,
            copied,
            skipped,
            MAX_UNTRACKED_FILES,
            MAX_UNTRACKED_BYTES >> 20
        );
    }
    wt.untracked_copied = copied as i32;
    wt.untracked_skipped = skipped as i32;

    // Commit the replayed state as a baseline so "did this task change
    // anything?" has an exact answer later. Without it the user's own
    // uncommitted work counts as a change: a read-only task on a repo with an
    // untracked scratch file would auto-commit that file at the end and leave
    // behind a branch the agent never touched. The baseline also makes the
    // delivered branch readable — `git diff <baseline>..<branch>` is precisely
    // the agent's work, with the user's WIP as its own labelled commit.
    let dirty = match worktree_is_dirty(&worktree_path).await {
        Ok(dirty) => dirty,
        Err(e) => {
            let _ = remove_local_worktree_dir(&git_root, &worktree_path).await;
            let _ = delete_branch(&git_root, &actual_branch).await;
            return Err(anyhow!(
                "execenv: could not inspect the prepared worktree for {:?}: {e:#}",
                git_root
            ));
        }
    };
    if dirty {
        let baseline = match commit_baseline(&worktree_path).await {
            Ok(baseline) => baseline,
            Err(e) => {
                let _ = remove_local_worktree_dir(&git_root, &worktree_path).await;
                let _ = delete_branch(&git_root, &actual_branch).await;
                return Err(anyhow!(
                    "execenv: could not record a baseline commit for the replayed state of {:?}: {e:#}",
                    git_root
                ));
            }
        };
        wt.base_commit = baseline;
    }

    // Note on keeping sidecars out of the delivered branch: we deliberately do
    // NOT write .git/info/exclude here. A linked worktree reads info/exclude
    // from the repo's COMMON git dir, so the only file that would take effect
    // is the user's own .git/info/exclude — editing it would change what `git
    // status` shows in the user's checkout, which is theirs, not ours. Instead
    // the daemon runs the existing CleanupRuntimeConfig + CleanupSidecars pass
    // over the worktree before Finalize, so the sidecars are simply gone by the
    // time anything is committed. That also preserves a genuine agent edit to a
    // tracked CLAUDE.md, which a blanket exclude would have swallowed.

    tracing::info!(
        git_root = %git_root,
        path = %worktree_path,
        branch = %actual_branch,
        base = %wt.base_commit,
        dirty_base_captured = wt.dirty_base_captured,
        untracked_copied = copied,
        untracked_skipped = skipped,
        "execenv: local worktree ready"
    );
    Ok(wt)
}

// ---------------------------------------------------------------------------
// Finalize / Discard / Abort
// ---------------------------------------------------------------------------

impl LocalWorktree {
    /// Finalize commits whatever the agent left behind, removes the worktree,
    /// and reports the branch. Called after the agent exits, before the env
    /// root is handed to the GC.
    ///
    /// The auto-commit is the reason a worktree task can't lose work: `git
    /// worktree remove --force` would happily delete uncommitted edits, and
    /// the user would have no way to get them back. Committing first turns
    /// "the agent edited files" into "the branch has a commit", which is the
    /// delivery contract for this mode.
    ///
    /// If that commit cannot be made — a repo with commit.gpgSign and no
    /// signing key available to the daemon, a full disk, a ref lock we lost —
    /// Finalize returns an error and DELIBERATELY LEAVES THE WORKTREE IN PLACE.
    /// Removing it would be the one operation in this file that destroys work
    /// with no way back, and a warning in the daemon log is not an acceptable
    /// substitute for the user's changes. The surviving worktree stays
    /// registered in the user's repo, so `git worktree list` points straight
    /// at it.
    pub async fn finalize(&self) -> anyhow::Result<LocalWorktreeOutcome> {
        let (_lock, _guard) = lock_git_root(&self.git_root).await;

        let mut outcome = LocalWorktreeOutcome {
            branch: self.branch.clone(),
            repo_identity: self.repo_identity.clone(),
            execution_workspace: self.path.clone(),
            head_state: "attached".to_string(),
            ..Default::default()
        };

        // Something before the commit went wrong in a way that would make the
        // delivered branch misleading. Commit nothing and keep the worktree:
        // the agent's work is still in it, and so is whatever the caller could
        // not clean up, which a human can now look at directly.
        if let Some(aborted) = &self.aborted {
            // Report NO branch. One exists in the user's repo, but nothing was
            // committed to it, so naming it as this task's result would point
            // them at a branch that is missing the very work they are looking
            // for. The preserved worktree path below is the honest pointer.
            outcome.branch = String::new();
            outcome.preserved_path = self.path.clone();
            tracing::error!(
                path = %self.path,
                branch = %self.branch,
                git_root = %self.git_root,
                error = %aborted,
                "execenv: worktree finalize aborted; nothing committed, worktree kept for inspection"
            );
            bail!(
                "refusing to deliver branch {}: {}; the task worktree is preserved at {} (listed by `git worktree list` in {})",
                self.branch,
                aborted,
                self.path,
                self.git_root
            );
        }

        // Treat "can't tell" like "dirty": committing costs an empty commit at
        // worst, while assuming clean risks deleting the agent's edits.
        let dirty = match worktree_is_dirty(&self.path).await {
            Ok(dirty) => dirty,
            Err(status_err) => {
                tracing::warn!(
                    path = %self.path,
                    error = %format!("{status_err:#}"),
                    "execenv: inspect worktree status failed; committing defensively"
                );
                true
            }
        };
        if dirty {
            match self.commit_all().await {
                Ok(committed) => outcome.auto_committed = committed,
                Err(err) => {
                    outcome.preserved_path = self.path.clone();
                    tracing::error!(
                        path = %self.path,
                        branch = %self.branch,
                        git_root = %self.git_root,
                        error = %format!("{err:#}"),
                        "execenv: could not commit the agent's changes; keeping the worktree so the work is recoverable"
                    );
                    bail!(
                        "could not commit the agent's changes to branch {}: {err:#}; the work is preserved in the worktree at {} (listed by `git worktree list` in {}) — recover it before that directory is reclaimed",
                        self.branch,
                        self.path,
                        self.git_root
                    );
                }
            }
        }

        // A branch still sitting exactly on its base commit means the task changed
        // nothing — the read-only case. Delete it so the user's branch list only
        // ever grows for tasks that actually produced work.
        let tip = run_git_trimmed(&self.path, ["rev-parse", "--verify", "HEAD"]).await;
        outcome.head_sha = tip.as_ref().map(|value| value.clone()).unwrap_or_default();
        let produced_work = match &tip {
            Err(_) => true,
            Ok(tip) => *tip != self.base_commit,
        };

        if let Err(remove_err) = remove_local_worktree_dir(&self.git_root, &self.path).await {
            outcome.preserved_path = self.path.clone();
            bail!(
                "could not remove finalized worktree for branch {}: {remove_err:#}; the task worktree remains at {}",
                self.branch,
                self.path
            );
        }

        if !produced_work {
            let _ = delete_branch(&self.git_root, &self.branch).await;
            outcome.branch = String::new();
            // The branch was deleted because this read-only task has no
            // deliverable. Preserve that terminal fact for discovery: a
            // start-time attached branch must not be rediscovered after it is
            // removed.
            outcome.head_state = "detached".to_string();
        }

        tracing::info!(
            git_root = %self.git_root,
            branch = %outcome.branch,
            auto_committed = outcome.auto_committed,
            produced_work = produced_work,
            "execenv: local worktree finalized"
        );
        Ok(outcome)
    }

    /// Discard tears a worktree down without delivering anything: unregister
    /// it, delete its directory, drop its branch.
    ///
    /// For the abandon-before-the-agent-ran case only. Finalize is the path
    /// that preserves work; this one assumes there is none to preserve, so
    /// callers must be sure nothing has run in the worktree yet.
    pub async fn discard(&self) {
        let (_lock, _guard) = lock_git_root(&self.git_root).await;
        let _ = remove_local_worktree_dir(&self.git_root, &self.path).await;
        let _ = delete_branch(&self.git_root, &self.branch).await;
        tracing::info!(
            git_root = %self.git_root,
            path = %self.path,
            branch = %self.branch,
            "execenv: local worktree discarded before the agent ran"
        );
    }

    /// AbortWithReason marks the worktree undeliverable. Finalize will then
    /// commit nothing, remove nothing, and return an error naming the
    /// preserved path.
    ///
    /// This exists because the decision "is this branch safe to deliver?" is
    /// made outside this package — the daemon knows whether its own sidecar
    /// cleanup succeeded — while the only code that can act on it is Finalize.
    /// The first reason wins: it is the one closest to the root cause.
    pub fn abort_with_reason(&mut self, err: &anyhow::Error) {
        if self.aborted.is_some() {
            return;
        }
        self.aborted = Some(format!("{err:#}"));
    }

    /// commitAll stages and commits everything the agent left behind. Returns
    /// whether a commit was actually created; an error means the changes are
    /// still only on disk and the caller must not delete the worktree.
    async fn commit_all(&self) -> anyhow::Result<bool> {
        commit_everything(&self.path, "chore(agent): uncommitted changes from task")
            .await
            .map(|(committed, _)| committed)
    }
}

// ---------------------------------------------------------------------------
// Commits
// ---------------------------------------------------------------------------

/// commitBaseline records the user's replayed uncommitted state as the first
/// commit on the task branch, returning the new tip.
async fn commit_baseline(worktree_path: &str) -> anyhow::Result<String> {
    let (_, created) = commit_everything(
        worktree_path,
        "chore(agent): baseline — uncommitted work from the local directory",
    )
    .await?;
    let _ = created;
    let tip = run_git_trimmed(worktree_path, ["rev-parse", "--verify", "HEAD"])
        .await
        .map_err(|e| anyhow!("resolve baseline commit: {e:#}"))?;
    Ok(tip)
}

/// commitEverything returns Ok(false) for the benign "there was nothing to
/// commit" case and an error for a real failure — the distinction callers
/// need to decide whether the tree is safe to discard.
async fn commit_everything(worktree_path: &str, message: &str) -> anyhow::Result<(bool, bool)> {
    if let Err((out, add_err)) = run_git(worktree_path, ["add", "-A"]).await {
        bail!("git add: {}: {add_err:#}", out.trim());
    }
    // --no-verify: the user's commit hooks are written for the user's own
    // workflow (interactive linters, test suites, signing prompts) and a hook
    // failure here would mean losing the agent's work to save a lint run. Note
    // it does NOT disable commit.gpgSign, which is why the caller has to treat
    // a commit failure as "keep the worktree" rather than a warning.
    let identity = commit_identity_args(worktree_path).await;
    let mut args: Vec<&str> = Vec::with_capacity(identity.len() + 5);
    args.extend(identity.iter().map(String::as_str));
    args.extend(["commit", "--no-verify", "-m", message]);
    match run_git(worktree_path, args).await {
        Ok(_) => Ok((true, true)),
        Err((out, commit_err)) => {
            if out.contains("nothing to commit") {
                return Ok((false, false));
            }
            bail!("git commit: {}: {commit_err:#}", out.trim());
        }
    }
}

/// commit_identity_args supplies a committer identity only when the repo
/// doesn't already have one. A repo with user.email configured keeps it, so
/// commits still look like they came from the user's own setup.
async fn commit_identity_args(dir: &str) -> Vec<String> {
    if let Ok(email) = run_git_trimmed(dir, ["config", "user.email"]).await {
        if !email.is_empty() {
            return Vec::new();
        }
    }
    vec![
        "-c".to_string(),
        "user.name=Patchbay Agent".to_string(),
        "-c".to_string(),
        "user.email=agent@patchbay.local".to_string(),
    ]
}

async fn worktree_is_dirty(worktree_path: &str) -> anyhow::Result<bool> {
    let (out, _) = run_git(worktree_path, ["status", "--porcelain"])
        .await
        .map_err(|(_, e)| anyhow!("git status: {e:#}"))?;
    Ok(!out.trim().is_empty())
}

// ---------------------------------------------------------------------------
// Worktree removal / branches
// ---------------------------------------------------------------------------

/// remove_local_worktree_dir unregisters the worktree from the user's repo and
/// deletes its directory. The branch is deliberately left alone — it is the
/// task's deliverable.
async fn remove_local_worktree_dir(git_root: &str, worktree_path: &str) -> anyhow::Result<()> {
    let mut remove_err: Option<String> = None;
    if let Err((out, err)) =
        run_git(git_root, ["worktree", "remove", "--force", worktree_path]).await
    {
        tracing::warn!(
            path = %worktree_path,
            output = %out.trim(),
            error = %format!("{err:#}"),
            "execenv: git worktree remove failed; pruning registration"
        );
        remove_err = Some(format!("{err:#}"));
        // Fall back to deleting the directory ourselves and dropping the now
        // dangling registration, so the user's repo isn't left listing a
        // worktree that no longer exists.
        if let Err(rm_err) = remove_tree_recursive(Path::new(worktree_path)) {
            let merged = match &remove_err {
                Some(prev) => format!("{prev}; {rm_err}"),
                None => rm_err.clone(),
            };
            remove_err = Some(merged);
            tracing::warn!(
                path = %worktree_path,
                error = %rm_err,
                "execenv: remove worktree directory failed"
            );
        }
        if let Err((out, prune_err)) = run_git(git_root, ["worktree", "prune"]).await {
            tracing::warn!(output = %out.trim(), error = %prune_err, "execenv: git worktree prune failed");
        }
    }
    // Lstat verifies the path entry itself is gone. Stat would treat a broken
    // symlink as absent even though a stale entry still occupies the handoff path.
    match Path::new(worktree_path).symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            bail!("could not confirm worktree removal for {worktree_path:?}: {e}")
        }
        Ok(_) => {}
    }
    if let Some(remove_err) = remove_err {
        bail!("worktree directory still exists after removal fallback: {remove_err}");
    }
    bail!("worktree directory still exists after git removal reported success")
}

/// Best-effort recursive delete standing in for os.RemoveAll.
fn remove_tree_recursive(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
        Ok(md) if md.file_type().is_symlink() || md.is_file() => {
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
        Ok(_) => std::fs::remove_dir_all(path).map_err(|e| e.to_string()),
    }
}

/// delete_branch drops a task branch that carries nothing worth keeping — an
/// empty read-only run, or a prepare that aborted partway. Best-effort: a
/// leftover branch is untidy, never harmful.
async fn delete_branch(git_root: &str, branch: &str) -> anyhow::Result<()> {
    if branch.is_empty() {
        return Ok(());
    }
    if let Err((out, err)) = run_git(git_root, ["branch", "-D", branch]).await {
        tracing::warn!(
            branch = %branch,
            output = %out.trim(),
            error = %format!("{err:#}"),
            "execenv: delete task branch failed (non-fatal)"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolution helpers
// ---------------------------------------------------------------------------

/// resolve_git_root returns the repository root containing dir. Worktree mode
/// is opt-in per resource, so a non-git directory here is a misconfiguration
/// the user needs to see and fix — we fail closed with an actionable message
/// rather than silently degrading to the in-place lock, which would leave the
/// user wondering why their tasks still queue.
async fn resolve_git_root(dir: &str) -> anyhow::Result<String> {
    let root = run_git_trimmed(dir, ["rev-parse", "--show-toplevel"]).await;
    let root = match root {
        Ok(root) if !root.is_empty() => root,
        _ => bail!(
            "execenv: local_directory {dir:?} is not a git repository, \
             but its project resource is set to execution_mode=worktree; \
             initialise a repository there or switch the resource back to in_place"
        ),
    };
    // Canonicalise so the root matches the path git reports from inside the
    // worktree later — on macOS /tmp vs /private/tmp otherwise produce two
    // different lock keys for one repo.
    let root = std::fs::canonicalize(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(root);
    Ok(clean_path_unix(&root))
}

/// Lexical filepath.Clean for a rooted path (resolve_git_root post-canonical).
fn clean_path_unix(path: &str) -> String {
    let cleaned: PathBuf = Path::new(path)
        .components()
        .fold(PathBuf::new(), |mut acc, c| {
            use std::path::Component::*;
            match c {
                CurDir => {}
                other => acc.push(other.as_os_str()),
            }
            acc
        });
    cleaned.to_string_lossy().into_owned()
}

/// add_local_worktree creates the worktree, retrying once under a suffixed
/// branch name when the branch already exists (a re-dispatched task keeps its
/// id, so its branch can survive from the previous run).
async fn add_local_worktree(
    git_root: &str,
    worktree_path: &str,
    branch: &str,
    base_ref: &str,
) -> anyhow::Result<String> {
    let mut branch = branch.to_string();
    let mut attempt = run_git(
        git_root,
        ["worktree", "add", "-b", &branch, worktree_path, base_ref],
    )
    .await;
    if let Err((out, _)) = &attempt {
        if out.to_lowercase().contains("already exists") {
            branch = format!("{branch}-{}", chrono::Utc::now().timestamp());
            attempt = run_git(
                git_root,
                ["worktree", "add", "-b", &branch, worktree_path, base_ref],
            )
            .await;
        }
    }
    match attempt {
        Ok(_) => Ok(branch),
        Err((out, err)) => bail!("execenv: git worktree add: {}: {err:#}", out.trim()),
    }
}

// ---------------------------------------------------------------------------
// Untracked-file replay
// ---------------------------------------------------------------------------

/// copy_untracked_files replays the user's untracked-but-not-ignored files
/// into the worktree. `git worktree add` only materialises committed content,
/// so without this a brand-new file the user just created would be invisible
/// to the agent. Bounded by maxUntrackedFiles / maxUntrackedBytes; the number
/// skipped is returned so the caller can tell the user instead of quietly
/// under-copying. Mirrors Go's behaviour of logging per-file problems at warn
/// level and counting them as skips where Go does.
async fn copy_untracked_files(git_root: &str, worktree_path: &str) -> (usize, usize) {
    // stdout only: a warning on stderr would otherwise be split apart and
    // treated as file paths to copy. Raw, not trimmed: with -z the entries are
    // exact filenames, and a file whose name begins or ends with whitespace
    // would be trim-corrupted into a path that fails to stat and silently
    // vanishes from the replay.
    let out = match run_git_stdout(
        git_root,
        ["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await
    {
        Ok(out) => out,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "execenv: git ls-files failed");
            return (0, 1);
        }
    };

    let mut budget = MAX_UNTRACKED_BYTES;
    let mut copied = 0usize;
    let mut skipped = 0usize;
    for rel in out.split('\u{0}') {
        if rel.is_empty() {
            continue;
        }
        // Never replay Patchbay's own sidecars. They are untracked files in the
        // user's directory whenever an in_place task is mid-flight on the same
        // path, or was killed before its cleanup ran. Copying them would put
        // another issue's brief inside this task's worktree — where the agent
        // would read it as its own context — and commit it to the branch.
        if is_patchbay_sidecar_path(rel) {
            continue;
        }
        if copied >= MAX_UNTRACKED_FILES || budget <= 0 {
            skipped += 1;
            continue;
        }
        let src = join_path(&[git_root, rel]);
        let info = match std::fs::symlink_metadata(&src) {
            Ok(info) => info,
            // Listed a moment ago, unreadable now — the tree changed under us,
            // so the snapshot no longer matches what the user has. Counted, not
            // skipped silently: the caller fails the task on a non-zero count.
            Err(stat_err) => {
                skipped += 1;
                tracing::warn!(
                    file = %rel,
                    error = %stat_err,
                    "execenv: untracked file vanished between listing and copy"
                );
                continue;
            }
        };
        if info.file_type().is_symlink() {
            // An untracked symlink is content the user can see. Reproducing it
            // faithfully means deciding whether to copy the link or its target
            // — including targets outside the repo — so this replay does not
            // try. Count it so the task fails rather than handing the agent a
            // tree with a file quietly missing.
            skipped += 1;
            tracing::warn!(file = %rel, "execenv: untracked symlink not replayed into worktree");
            continue;
        }
        if !info.is_file() {
            // Sockets, FIFOs, devices: not content, and not something an agent
            // can meaningfully read from a copy. Skipping them does not make the
            // snapshot misleading, so this one stays uncounted.
            continue;
        }
        let size = info.len() as i64;
        if size > budget {
            skipped += 1;
            continue;
        }
        let dst = join_path(&[worktree_path, rel]);
        if let Err(copy_err) = copy_untracked_file(&src, &dst, &info) {
            skipped += 1;
            tracing::warn!(
                file = %rel,
                error = %copy_err,
                "execenv: copy untracked file into worktree failed"
            );
            continue;
        }
        budget -= size;
        copied += 1;
    }
    (copied, skipped)
}

/// patchbaySidecarDirNames are the directories Prepare writes into a workdir. A
/// task running in_place on the same directory leaves these present as
/// untracked files for the length of its run, so a concurrent worktree snapshot
/// sees them. CLAUDE.md / AGENTS.md are deliberately absent: those are
/// ordinarily the user's own tracked files, and the runtime only injects a
/// marker block into them, which CleanupRuntimeConfig removes.
const PATCHBAY_SIDECAR_DIR_NAMES: [&str; 2] = [".agent_context", ".patchbay"];

/// is_patchbay_sidecar_path reports whether a repo-relative path is one of the
/// daemon's own sidecars rather than the user's content. Matched as a whole
/// path segment at ANY depth, not just the repo root: an in_place resource may
/// point at a subdirectory of this repo, in which case its sidecars sit at
/// <subdir>/.agent_context — replaying those would put another issue's brief
/// inside this task's worktree and commit it to the delivered branch.
pub(crate) fn is_patchbay_sidecar_path(rel: &str) -> bool {
    rel.split('/')
        .any(|seg| PATCHBAY_SIDECAR_DIR_NAMES.contains(&seg))
}

fn copy_untracked_file(src: &str, dst: &str, md: &std::fs::Metadata) -> anyhow::Result<()> {
    if let Some(parent) = Path::new(dst).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dst,
            std::fs::Permissions::from_mode(md.permissions().mode() & 0o777),
        )?;
    }
    #[cfg(windows)]
    {
        let _ = md;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Git plumbing
// ---------------------------------------------------------------------------

/// run_git runs git in dir and returns combined output. Callers inspect the
/// output for git's own error text, so stdout and stderr stay merged.
/// Errors surface as `(combined_output, error)` pairs the way Go callers read
/// `CombinedOutput()` alongside the error.
async fn run_git<I>(dir: &str, args: I) -> Result<(String, String), (String, anyhow::Error)>
where
    I: IntoIterator<Item: AsRef<str>>,
{
    let mut cmd = Command::new("git");
    cmd.kill_on_drop(true);
    cmd.arg("-C").arg(dir);
    for a in args {
        cmd.arg(a.as_ref());
    }
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await;
    match out {
        Err(_) => {
            let err = anyhow!("git invocation timed out after {:?}", GIT_TIMEOUT);
            Err((String::new(), err))
        }
        Ok(Err(e)) => Err((String::new(), anyhow::Error::new(e))),
        Ok(Ok(out)) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if out.status.success() {
                Ok((combined, String::new()))
            } else {
                Err((combined, anyhow!("exit status {}", out.status)))
            }
        }
    }
}

/// run_git_trimmed runs git for its stdout value, discarding stderr so a
/// diagnostic line can't be mistaken for the value (`rev-parse` output, a
/// config value, a stash sha).
async fn run_git_trimmed<I>(dir: &str, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item: AsRef<str>>,
{
    let out = run_git_stdout(dir, args).await?;
    Ok(out.trim().to_string())
}

/// run_git_stdout is run_git_trimmed without the trimming, for output where
/// whitespace is significant — NUL-separated file listings, where a leading or
/// trailing space is part of a filename.
async fn run_git_stdout<I>(dir: &str, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item: AsRef<str>>,
{
    let mut cmd = Command::new("git");
    cmd.kill_on_drop(true);
    cmd.arg("-C").arg(dir);
    for a in args {
        cmd.arg(a.as_ref());
    }
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await??;
    if !out.status.success() {
        bail!("git exited with status {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// path_rel ports Go's filepath.Rel for the rooted cases this file needs:
/// returns the lexical relative path from base to target, or None when target
/// escapes base.
fn path_rel(base: &str, target: &str) -> Option<String> {
    let b = Path::new(base);
    let t = Path::new(target);
    t.strip_prefix(b).ok().map(|p| {
        let s = p.to_string_lossy().into_owned();
        if s.is_empty() {
            ".".to_string()
        } else {
            super::execenv::clean_path(&s)
        }
    })
}
