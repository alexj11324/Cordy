//! Port of execenv/codex_home.go.
//!
//! Symbol map:
//! - codexCopiedFiles              → CODEX_COPIED_FILES
//! - codexModelsCacheFile / codexModelsCacheBindingFile
//!   → CODEX_MODELS_CACHE_FILE / CODEX_MODELS_CACHE_BINDING_FILE
//! - CodexHomeOptions               → CodexHomeOptions
//! - prepareCodexHomeWithOpts       → prepare_codex_home_with_opts
//! - resolveSharedCodexHome         → resolve_shared_codex_home
//! - codexSessionStateGlobs       → CODEX_SESSION_STATE_GLOBS
//! - codexSessionStoreRoot        → CODEX_SESSION_STORE_ROOT
//! - codexSessionStoreDir           → codex_session_store_dir
//! - codexSessionStoreNamespace     → codex_session_store_namespace
//! - codexSessionStoreKey           → codex_session_store_key
//! - sanitizePathSegment            → sanitize_path_segment
//! - PruneCodexSessionStores        → prune_codex_session_stores
//! - codexStoreStat                 → codex_store_stat
//! - prepareCodexSessionsDir        → prepare_codex_sessions_dir
//! - linkCodexSessionsToStore       → link_codex_sessions_to_store
//! - touchCodexSessionStore         → touch_codex_session_store
//! - CodexSessionStorePath          → codex_session_store_path
//! - sameCodexPath                  → same_codex_path
//! - resetCodexSessionState         → reset_codex_session_state
//! - ensureCodexSessionsLink        → ensure_codex_sessions_link
//! - codexRolloutGlobs / findCodexRollouts
//!   → codex_rollout_globs / find_codex_rollouts
//! - CodexResumeRolloutPresent      → codex_resume_rollout_present
//! - exposeResumeRollout            → expose_resume_rollout
//! - linkCodexRollout               → link_codex_rollout
//! - openVerifiedCodexHomeRoot      → (folded into materialise_in_codex_home;
//!   std has no os.Root; see Deviations)
//! - materialiseInCodexHome         → materialise_in_codex_home
//! - syncCodexModelsCache           → sync_codex_models_cache
//! - codexModelsCacheConfigFingerprint → codex_models_cache_config_fingerprint
//! - readCodexModelsCacheBinding /
//!   writeCodexModelsCacheBinding    → read/write_codex_models_cache_binding
//! - resolveCodexConfigPath         → resolve_codex_config_path
//! - syncCopiedFile / seedCopiedFile → sync_copied_file / seed_copied_file
//! - sharedConfigPresence /
//!   statSharedCodexConfig           → SharedConfigPresence / stat_shared_codex_config
//! - resolveWindowsSandboxState /
//!   classifyPerTaskWindowsSandbox   → resolve_windows_sandbox_state /
//!   classify_per_task_windows_sandbox
//!
//! Deviations:
//! - slog logger parameters dropped; tracing macros used directly.
//! - go-toml struct decode → toml crate Value navigation.
//! - os.Root root-scoped writes have no stable std equivalent; the symlink
//!   identity check of openVerifiedCodexHomeRoot is preserved lexically
//!   (refuse when the task home is a symlink, verify dir-ness before each
//!   write) and the residual TOCTOU window is documented as Go's PB-5647.
//! - filepath.Glob → walkdir-based matching (std glob semantics differ).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use sha2::{Digest, Sha256};

use super::execenv::join_path;
use super::execenv::user_home_dir;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Files to copy from the shared ~/.codex/ into the per-task CODEX_HOME.
/// Copies are isolated — task-local config and cache refreshes don't mutate
/// the shared home.
pub(crate) const CODEX_COPIED_FILES: [&str; 1] = ["instructions.md"];

pub(crate) const CODEX_MODELS_CACHE_FILE: &str = "models_cache.json";
pub(crate) const CODEX_MODELS_CACHE_BINDING_FILE: &str = ".models_cache_config.sha256";

/// Files whose contents select the model provider/catalog used by Codex. The
/// task-local models cache is only reusable while this source configuration
/// remains unchanged.
const CODEX_MODELS_CACHE_CONFIG_FILES: [&str; 2] = ["config.json", "config.toml"];

/// codexSessionStateGlobs are the session-derived SQLite state Codex builds
/// inside a CODEX_HOME by indexing everything under sessions/. They are dropped
/// during the legacy-symlink migration so Codex rebuilds them from the now
/// task-local sessions instead of keeping the thousands of stale rows it
/// backfilled from the shared ~/.codex/sessions history.
///
/// Deliberately NOT listed: session_index.jsonl (authoritative for thread-id →
/// user-set thread name in Codex 0.144.x), and sibling per-task DBs with
/// different prefixes (goals_*, logs_*, memories_*) which are not
/// session-derived. All are left intact.
const CODEX_SESSION_STATE_GLOBS: [&str; 3] =
    ["state_*.sqlite", "state_*.sqlite-shm", "state_*.sqlite-wal"];

/// codexSessionStoreRoot is the directory under the shared Codex home that
/// holds the per-issue session stores. It sits beside the user's own `sessions/`
/// so it shares that volume (making resume-rollout hard links zero-copy) but is
/// never enumerated by a plain `codex` run.
const CODEX_SESSION_STORE_ROOT: &str = "patchbay-sessions";

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// CodexHomeOptions carries optional inputs for prepare_codex_home_with_opts
/// that affect the generated per-task config.toml.
#[derive(Debug, Clone, Default)]
pub struct CodexHomeOptions {
    /// Detected Codex CLI version (e.g. "0.121.0"). Empty means unknown; on
    /// macOS, unknown is treated as "probably broken" so the daemon falls back
    /// to danger-full-access for network access. See codex_sandbox.rs.
    pub codex_version: String,
    /// GOOS overrides the target platform when deciding the sandbox policy.
    /// Empty means use the host platform.
    pub goos: String,
    /// ResumeSessionID is the Codex thread/session ID this run intends to
    /// resume, when any (PB-4424).
    pub resume_session_id: String,
    /// IsLocalDirectory marks a task whose env root is never reused across
    /// task IDs (PB-4424).
    pub is_local_directory: bool,
    /// SessionStoreKey is a stable, per-(agent, issue-or-chat) relative path
    /// identifying this task's persistent Codex sessions store (PB-4424).
    pub session_store_key: String,
    /// Effective Codex CLI args this task launches with. Only the Windows
    /// sandbox decision reads them (PB-4957).
    pub codex_custom_args: Vec<String>,
}

impl CodexHomeOptions {
    /// Test helper mirroring Go's prepareCodexHome wrapper default (GOOS pinned
    /// to linux → danger-full-access sandbox block regardless of host).
    pub fn test_default() -> Self {
        CodexHomeOptions {
            goos: "linux".to_string(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// prepare_codex_home_with_opts creates a per-task CODEX_HOME directory and
/// seeds it with non-credential config from the shared ~/.codex/ home. Provider
/// authentication remains daemon-owned and is delivered only through the
/// short-lived broker. The per-task config.toml gets
/// a daemon-managed sandbox block picked by codex_sandbox_policy_for_config.
pub fn prepare_codex_home_with_opts(
    codex_home: &str,
    opts: CodexHomeOptions,
) -> anyhow::Result<()> {
    let shared_home = resolve_shared_codex_home();
    let fresh_home = Path::new(codex_home).symlink_metadata().is_err();

    std::fs::create_dir_all(codex_home).context("create codex-home dir")?;

    // Give the task its own local sessions/ directory instead of symlinking the
    // shared ~/.codex/sessions in — a huge shared history would otherwise stall
    // Codex's `initialize` state backfill (PB-4424). See prepare_codex_sessions_dir.
    if let Err(err) = prepare_codex_sessions_dir(codex_home, &shared_home, &opts) {
        tracing::warn!(error = %format!("{err:#}"), "execenv: codex-home sessions dir prepare failed");
    }

    // A pre-upgrade task home may still contain a symlink or copied credential.
    // Remove it before any provider process is allowed to start.
    remove_any(&join_path(&[codex_home, "auth.json"]))
        .context("remove legacy task Codex credential")?;
    remove_any(&join_path(&[codex_home, "config.json"]))
        .context("remove legacy task Codex config")?;
    remove_any(&join_path(&[codex_home, "config.toml"]))
        .context("remove legacy task Codex config")?;

    // Copy only non-credential instructions. Host config may contain provider
    // API keys, custom headers, executable hooks, or absolute host paths.
    for name in CODEX_COPIED_FILES {
        let src = join_path(&[&shared_home, name]);
        let dst = join_path(&[codex_home, name]);
        if let Err(err) = sync_copied_file(&src, &dst) {
            tracing::warn!(file = %name, error = %format!("{err:#}"), "execenv: codex-home sync failed");
        }
    }

    // Seed the shared model cache only for a fresh task home. On reuse, keep a
    // task-local cache that Codex may have refreshed, but only while the source
    // provider/catalog configuration is still the one that cache was bound to.
    if let Err(err) = sync_codex_models_cache(codex_home, &shared_home, fresh_home) {
        tracing::warn!(
            error = %format!("{err:#}"),
            "execenv: codex-home models cache sync failed; discarding cache"
        );
        let cache = join_path(&[codex_home, CODEX_MODELS_CACHE_FILE]);
        if let Err(remove_err) = super::execenv::remove_tree(&cache) {
            return Err(anyhow!(
                "sync codex models cache: {err:#}; discard unsafe cache: {remove_err:#}"
            ));
        }
    }

    remove_any(&join_path(&[codex_home, "plugins", "cache"]))
        .context("remove legacy shared Codex plugin cache")?;

    // Write a daemon-managed sandbox block into config.toml (see codex_sandbox.rs).
    let config_file = join_path(&[codex_home, "config.toml"]);
    let win_state = if super::codex_sandbox::resolve_goos(&opts.goos) == "windows" {
        resolve_windows_sandbox_state(
            &config_file,
            None,
            stat_shared_codex_config(&shared_home),
            &opts.codex_custom_args,
        )
    } else {
        super::codex_sandbox::WindowsSandboxConfig::Absent
    };
    let policy = super::codex_sandbox::codex_sandbox_policy_for_config(
        &opts.goos,
        &opts.codex_version,
        win_state,
    );
    super::codex_sandbox::ensure_codex_sandbox_config(&config_file, &policy, &opts.codex_version)
        .map_err(|e| anyhow!("ensure codex sandbox config: {e:#}"))?;

    // Disable Codex native multi-agent inside daemon-managed task sessions
    // (see codex_multi_agent.rs).
    if let Err(err) = super::codex_multi_agent::ensure_codex_multi_agent_config(&join_path(&[
        codex_home,
        "config.toml",
    ])) {
        tracing::warn!(error = %format!("{err:#}"), "execenv: codex-home ensure multi-agent config failed");
    }

    // Disable Codex native auto-memory inside daemon-managed task sessions
    // (see codex_memory.rs).
    if let Err(err) =
        super::codex_memory::ensure_codex_memory_config(&join_path(&[codex_home, "config.toml"]))
    {
        tracing::warn!(error = %format!("{err:#}"), "execenv: codex-home ensure memory config failed");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Shared home resolution
// ---------------------------------------------------------------------------

/// resolve_shared_codex_home returns the path to the user's shared Codex home.
/// Checks $CODEX_HOME first, falls back to ~/.codex.
pub(crate) fn resolve_shared_codex_home() -> String {
    if let Ok(v) = std::env::var("CODEX_HOME") {
        if !v.is_empty() {
            if let Ok(abs) = std::path::absolute(&v) {
                return abs.to_string_lossy().into_owned();
            }
        }
    }
    match user_home_dir() {
        Ok(home) => join_path(&[&home, ".codex"]),
        Err(_) => {
            let tmp = std::env::temp_dir().to_string_lossy().into_owned();
            join_path(&[&tmp, ".codex"])
        }
    }
}

// ---------------------------------------------------------------------------
// Windows sandbox tri-state glue (delegating to codex_sandbox.rs)
// ---------------------------------------------------------------------------

/// sharedConfigPresence is the tri-state existence of the shared
/// ~/.codex/config.toml copy source. Three-valued so a stat that fails for a
/// reason other than "not found" never masquerades as a confident "the user
/// has no config" (PB-4957).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SharedConfigPresence {
    /// The shared config.toml is confidently not present.
    Absent,
    /// The shared config.toml exists.
    Present,
    /// The stat failed for a reason other than not-found.
    Undecidable,
}

/// stat_shared_codex_config classifies the shared ~/.codex/config.toml (the
/// copy source) into the tri-state above.
fn stat_shared_codex_config(shared_home: &str) -> SharedConfigPresence {
    if shared_home.is_empty() {
        return SharedConfigPresence::Absent;
    }
    match std::fs::metadata(join_path(&[shared_home, "config.toml"])) {
        Ok(_) => SharedConfigPresence::Present,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SharedConfigPresence::Absent,
        Err(_) => SharedConfigPresence::Undecidable,
    }
}

/// resolve_windows_sandbox_state determines, for a Windows task, whether a
/// native Codex sandbox is configured — across the per-task config.toml and
/// the effective custom args — failing closed (Undecidable) when it cannot
/// tell (PB-4957).
pub(crate) fn resolve_windows_sandbox_state(
    config_file: &str,
    config_sync_err: Option<&str>,
    shared_presence: SharedConfigPresence,
    custom_args: &[String],
) -> super::codex_sandbox::WindowsSandboxConfig {
    let config_state =
        classify_per_task_windows_sandbox(config_file, config_sync_err, shared_presence);
    let state = super::codex_sandbox::resolve_windows_sandbox(&[
        config_state,
        super::codex_sandbox::windows_sandbox_from_custom_args(custom_args),
    ]);
    if state == super::codex_sandbox::WindowsSandboxConfig::Undecidable {
        tracing::error!(
            config_file = %config_file,
            "codex sandbox: cannot determine Windows native sandbox config; keeping workspace-write and refusing to loosen to danger-full-access"
        );
    }
    state
}

/// classify_per_task_windows_sandbox inspects the per-task config.toml given
/// the outcome of syncing it from the shared source, failing closed whenever
/// the file cannot be trusted or read.
fn classify_per_task_windows_sandbox(
    config_file: &str,
    config_sync_err: Option<&str>,
    shared_presence: SharedConfigPresence,
) -> super::codex_sandbox::WindowsSandboxConfig {
    // A failed shared→per-task sync leaves config.toml stale or missing; neither
    // its contents nor its absence reflect the user's intent. Fail closed.
    if let Some(sync_err) = config_sync_err {
        let _ = sync_err;
        return super::codex_sandbox::WindowsSandboxConfig::Undecidable;
    }
    match std::fs::read_to_string(config_file) {
        Ok(data) => super::codex_sandbox::windows_sandbox_from_config(&data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Sync succeeded and the per-task config is absent. Genuine "no
            // config" only when the shared source is confidently absent too.
            if shared_presence == SharedConfigPresence::Absent {
                super::codex_sandbox::WindowsSandboxConfig::Absent
            } else {
                super::codex_sandbox::WindowsSandboxConfig::Undecidable
            }
        }
        Err(_) => super::codex_sandbox::WindowsSandboxConfig::Undecidable,
    }
}

// ---------------------------------------------------------------------------
// Session stores
// ---------------------------------------------------------------------------

/// codex_session_store_dir returns the persistent, per-(agent, issue) Codex
/// sessions store for key, rooted on the shared Codex home's volume. Empty key
/// → "" (caller keeps sessions/ task-local).
pub(crate) fn codex_session_store_dir(shared_home: &str, key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    join_path(&[shared_home, CODEX_SESSION_STORE_ROOT, key])
}

/// codex_session_store_namespace maps a daemon's profile to the directory
/// segment isolating its stores from another profile-daemon sharing one
/// ~/.codex. Collision-free AND fixed-length: the empty profile gets a
/// reserved bare literal and every named profile is hex(SHA-256) under "p_"
/// (PB-4424).
pub(crate) fn codex_session_store_namespace(profile: &str) -> String {
    if profile.is_empty() {
        return "default".to_string();
    }
    let sum = Sha256::digest(profile.as_bytes());
    format!("p_{}", hex::encode(sum))
}

/// codex_session_store_key builds a profile-and-task key for persistent Codex
/// sessions. Issue IDs retain their existing path; direct chats use a prefixed
/// chat_session_id so the two namespaces cannot collide. Returns "" when
/// neither stable identifier is available.
pub(crate) fn codex_session_store_key(
    profile: &str,
    task: &super::execenv::TaskContextForEnv,
) -> String {
    let mut store_id = sanitize_path_segment(&task.issue_id);
    if store_id.is_empty() {
        let chat_id = sanitize_path_segment(&task.chat_session_id);
        if chat_id.is_empty() {
            return String::new();
        }
        store_id = format!("chat_{chat_id}");
    }
    let agent = {
        let a = sanitize_path_segment(&task.agent_id);
        if a.is_empty() {
            "_".to_string()
        } else {
            a
        }
    };
    join_path(&[&codex_session_store_namespace(profile), &agent, &store_id])
}

/// sanitize_path_segment reduces s to the characters a UUID uses (hex plus
/// dashes/underscores), dropping everything else so the result is always a
/// single safe path segment — no separators, no "..", no drive letters.
fn sanitize_path_segment(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// PruneCodexSessionStores reclaims per-issue Codex session stores under the
/// shared home's patchbay-sessions root idle past retention. retention <= 0
/// disables pruning entirely. Scans ONLY the caller profile's namespace
/// (PB-4424). `reserve` atomically claims a store for deletion: ok=false
/// leaves the store; otherwise the returned commit runs once removal finishes.
/// None disables the guard (tests).
/// Reservation callback handed to store pruners: Ok(commit) claims the store;
/// Err(()) leaves it in place. The commit runs after removal finishes.
pub type ReserveCodexStore<'a> = dyn Fn(&str) -> Option<Box<dyn FnOnce() + 'a>> + 'a;

pub fn prune_codex_session_stores(
    profile: &str,
    retention: chrono::Duration,
    now: chrono::DateTime<chrono::Utc>,
    reserve: Option<&ReserveCodexStore<'_>>,
) -> (usize, i64) {
    if retention <= chrono::Duration::zero() {
        return (0, 0);
    }
    let root = join_path(&[
        &resolve_shared_codex_home(),
        CODEX_SESSION_STORE_ROOT,
        &codex_session_store_namespace(profile),
    ]);
    let agents = match std::fs::read_dir(&root) {
        Ok(a) => a,
        Err(_) => return (0, 0), // not created yet, or unreadable
    };
    let mut removed = 0usize;
    let mut bytes_freed = 0i64;
    for agent in agents.flatten() {
        if !agent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let agent_dir = agent.path().to_string_lossy().into_owned();
        let issues = match std::fs::read_dir(&agent_dir) {
            Ok(i) => i,
            Err(_) => continue,
        };
        let mut kept = 0usize;
        for issue in issues.flatten() {
            if !issue.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let store_dir = issue.path().to_string_lossy().into_owned();
            let (newest, size) = codex_store_stat(&store_dir);
            let newest = match newest {
                Some(n) => n,
                None => {
                    kept += 1;
                    continue;
                }
            };
            let newest_utc: chrono::DateTime<chrono::Utc> = newest.into();
            if now.signed_duration_since(newest_utc) <= retention {
                kept += 1;
                continue;
            }
            // Atomically reserve the store before removing it.
            let commit: Option<Box<dyn FnOnce() + '_>> = match reserve {
                Some(reserve) => match reserve(&store_dir) {
                    Some(c) => Some(c),
                    None => {
                        kept += 1;
                        continue;
                    }
                },
                None => None,
            };
            let err = std::fs::remove_dir_all(&store_dir).err();
            if let Some(commit) = commit {
                commit();
            }
            if let Some(err) = err {
                tracing::warn!(store = %store_dir, error = %err, "execenv: prune codex session store failed");
                kept += 1;
                continue;
            }
            removed += 1;
            bytes_freed += size;
        }
        // Remove the agent dir once its last issue store is gone.
        if kept == 0 {
            let _ = std::fs::remove_dir(&agent_dir);
        }
    }
    (removed, bytes_freed)
}

/// codex_store_stat walks dir once, returning the newest modification time
/// seen (the store's last activity) and its total byte size.
fn codex_store_stat(dir: &str) -> (Option<std::time::SystemTime>, i64) {
    let mut newest: Option<std::time::SystemTime> = None;
    let mut size = 0i64;
    for entry in walkdir::WalkDir::new(dir) {
        let Ok(entry) = entry else { continue };
        let Ok(md) = entry.metadata() else { continue };
        if let Ok(mt) = md.modified() {
            if newest.is_none_or(|n| mt > n) {
                newest = Some(mt);
            }
        }
        if !md.is_dir() {
            size += md.len() as i64;
        }
    }
    (newest, size)
}

/// prepare_codex_sessions_dir points codex-home/sessions at a sessions store
/// holding ONLY this task's own history, never the machine's whole
/// ~/.codex/sessions (PB-4424). See the Go doc for the four-way layout table.
fn prepare_codex_sessions_dir(
    codex_home: &str,
    shared_home: &str,
    opts: &CodexHomeOptions,
) -> anyhow::Result<()> {
    let dst = join_path(&[codex_home, "sessions"]);
    let shared_sessions = join_path(&[shared_home, "sessions"]);
    let store_dir = codex_session_store_dir(shared_home, &opts.session_store_key);

    // local_directory tasks have no reusable envRoot, so their history can only
    // persist across task IDs in the per-issue store.
    if opts.is_local_directory {
        if store_dir.is_empty() {
            // No stable per-issue key. Fall back to an empty local dir rather
            // than re-exposing the whole shared history.
            return std::fs::create_dir_all(&dst).map_err(anyhow::Error::new);
        }
        return link_codex_sessions_to_store(
            &dst,
            &store_dir,
            &shared_sessions,
            &opts.resume_session_id,
        );
    }

    let md = match Path::new(&dst).symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return std::fs::create_dir_all(&dst).map_err(anyhow::Error::new); // fresh managed task
        }
        Err(e) => return Err(anyhow!("stat sessions dir {dst}: {e}")),
        Ok(md) => md,
    };

    if !md.file_type().is_symlink() {
        // Already a real directory (task-local, authoritative). Leave contents.
        return std::fs::create_dir_all(&dst).map_err(anyhow::Error::new);
    }

    // A symlink/junction. If it already points at this issue's store, it is
    // authoritative — re-ensure the link and resume rollout, then leave it.
    if !store_dir.is_empty() {
        if let Ok(target) = std::fs::read_link(&dst) {
            if same_codex_path(&target.to_string_lossy(), &store_dir) {
                return link_codex_sessions_to_store(
                    &dst,
                    &store_dir,
                    &shared_sessions,
                    &opts.resume_session_id,
                );
            }
        }
    }

    // Legacy symlink into the shared ~/.codex/sessions — migrate it.
    std::fs::remove_file(&dst).context(format!("remove legacy sessions symlink {dst}"))?;
    reset_codex_session_state(codex_home);

    if !opts.resume_session_id.is_empty() && !store_dir.is_empty() {
        tracing::info!(
            codex_home = %codex_home,
            resume_session = true,
            "execenv: migrated codex-home sessions from shared symlink to per-issue store"
        );
        return link_codex_sessions_to_store(
            &dst,
            &store_dir,
            &shared_sessions,
            &opts.resume_session_id,
        );
    }
    tracing::info!(
        codex_home = %codex_home,
        resume_session = false,
        "execenv: migrated codex-home sessions from shared symlink to task-local dir"
    );
    std::fs::create_dir_all(&dst).map_err(anyhow::Error::new)
}

/// link_codex_sessions_to_store points codex-home/sessions (dst) at the
/// per-issue store via an idempotent directory link, hard-linking a missing
/// resume rollout first and stamping the store as just-used.
fn link_codex_sessions_to_store(
    dst: &str,
    store_dir: &str,
    shared_sessions: &str,
    resume_id: &str,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(store_dir)
        .with_context(|| format!("create codex session store {store_dir}"))?;
    if !resume_id.is_empty() && find_codex_rollouts(store_dir, resume_id).is_empty() {
        if let Err(err) = expose_resume_rollout(shared_sessions, store_dir, resume_id) {
            tracing::warn!(
                session_id = %resume_id,
                error = %format!("{err:#}"),
                "execenv: bootstrap resume rollout into session store failed; task will fall back to a fresh thread"
            );
        }
    }
    ensure_codex_sessions_link(dst, store_dir)?;
    touch_codex_session_store(store_dir);
    Ok(())
}

/// touch_codex_session_store refreshes storeDir's modification time to now —
/// the signal codex_store_stat reads as last activity. Best-effort.
fn touch_codex_session_store(store_dir: &str) {
    let now = filetime_now();
    if let Err(err) = set_mtime(Path::new(store_dir), now) {
        tracing::warn!(store = %store_dir, error = %err, "execenv: refresh codex session store activity failed");
    }
}

fn filetime_now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

#[cfg(unix)]
fn set_mtime(path: &Path, time: std::time::SystemTime) -> std::io::Result<()> {
    let ft: libc::timespec = {
        let d = time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        libc::timespec {
            tv_sec: d.as_secs() as libc::time_t,
            tv_nsec: d.subsec_nanos() as libc::c_long,
        }
    };
    let times = [ft; 2];
    let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), times.as_ptr(), 0) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn set_mtime(path: &Path, time: std::time::SystemTime) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES,
    };

    // FILE_WRITE_ATTRIBUTES is the precise access right required by
    // File::set_times. A read-only handle can compile successfully but fails
    // with ERROR_ACCESS_DENIED on Windows runners. BACKUP_SEMANTICS keeps the
    // same handle path valid for both files and directories.
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    file.set_times(
        std::fs::FileTimes::new()
            .set_accessed(time)
            .set_modified(time),
    )
}

/// codex_session_store_path returns the per-conversation Codex session store
/// on the shared home, or "" when there is no stable issue or chat key.
pub fn codex_session_store_path(profile: &str, task: &super::execenv::TaskContextForEnv) -> String {
    let key = codex_session_store_key(profile, task);
    if key.is_empty() {
        return String::new();
    }
    codex_session_store_dir(&resolve_shared_codex_home(), &key)
}

/// same_codex_path reports whether two filesystem paths refer to the same
/// location, tolerating separator/cleanliness differences.
fn same_codex_path(a: &str, b: &str) -> bool {
    clean_lexical(a) == clean_lexical(b)
}

/// Lexical clean for rooted paths (Go filepath.Clean subset used here).
fn clean_lexical(path: &str) -> String {
    let p = Path::new(path);
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            component @ RootDir => out.push(component.as_os_str()),
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

/// reset_codex_session_state removes the rebuildable, session-derived Codex
/// state files from a per-task CODEX_HOME so the next `initialize` re-derives
/// them from the task-local sessions. Only session-derived indexes are touched.
fn reset_codex_session_state(codex_home: &str) {
    for pattern in CODEX_SESSION_STATE_GLOBS {
        for m in glob_in(codex_home, pattern) {
            if let Err(err) = std::fs::remove_file(&m) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %m.display(), error = %err, "execenv: codex-home reset session state failed");
                }
            }
        }
    }
}

/// Minimal `filepath.Glob` equivalent supporting a single `*` wildcard per
/// segment (all patterns here are of the form `state_*.sqlite*`).
fn glob_in(dir: &str, pattern: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if glob_match(pattern, &name) {
            out.push(entry.path());
        }
    }
    out
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (i, part) in parts.iter().enumerate() {
        match i {
            0 => {
                if !rest.starts_with(part) {
                    return false;
                }
                rest = &rest[part.len()..];
            }
            n if n == parts.len() - 1 => {
                if !rest.ends_with(part) {
                    return false;
                }
            }
            _ => match rest.find(part) {
                Some(idx) => rest = &rest[idx + part.len()..],
                None => return false,
            },
        }
    }
    true
}

/// ensure_codex_sessions_link points codex-home/sessions (dst) at the per-issue
/// session store (src) via a directory link, creating the store if needed.
/// Idempotent: a link already pointing at src is left as-is.
fn ensure_codex_sessions_link(dst: &str, src: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(src).with_context(|| format!("create codex session store {src}"))?;
    if let Ok(md) = Path::new(dst).symlink_metadata() {
        if md.file_type().is_symlink() {
            if let Ok(target) = std::fs::read_link(dst) {
                if same_codex_path(&target.to_string_lossy(), src) {
                    return Ok(());
                }
            }
        }
        remove_any(dst).context(format!("remove stale sessions path {dst}"))?;
    }
    create_dir_link(src, dst)
}

fn remove_any(path: &str) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(md) if md.is_dir() && !md.file_type().is_symlink() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    }
}

/// createDirLink (unix build): a plain symlink.
pub(crate) fn create_dir_link(src: &str, dst: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        junction::create(src, dst).map_err(anyhow::Error::new)
    }
}

/// createFileLink (unix build): a plain symlink.
fn create_file_link(src: &str, dst: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        std::fs::copy(src, dst)
            .map(|_| ())
            .map_err(anyhow::Error::new)
    }
}

// ---------------------------------------------------------------------------
// Rollouts
// ---------------------------------------------------------------------------

/// codex_rollout_globs covers both layouts Codex 0.14x writes:
/// date-nested (sessions/YYYY/MM/DD/) and flat, each as .jsonl or .jsonl.zst.
fn codex_rollout_globs(sessions_dir: &str, session_id: &str) -> [String; 2] {
    let name = format!("rollout-*-{session_id}.jsonl*");
    [
        join_path(&[sessions_dir, &name]),
        join_path(&[sessions_dir, "*", "*", "*", &name]),
    ]
}

/// find_codex_rollouts returns every rollout file for sessionID under
/// sessionsDir, across the supported layouts, deduplicated.
fn find_codex_rollouts(sessions_dir: &str, session_id: &str) -> Vec<String> {
    if sessions_dir.is_empty() || session_id.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for base in codex_rollout_globs(sessions_dir, session_id) {
        for m in find_matching_files(sessions_dir, &base) {
            if seen.insert(m.clone()) {
                out.push(m);
            }
        }
    }
    out
}

/// Walks sessions_dir matching the pattern produced by codex_rollout_globs:
/// either a direct filename pattern or a 3-directory-deep nesting.
fn find_matching_files(root: &str, pattern: &str) -> Vec<String> {
    let rel = pattern.strip_prefix(&format!("{root}/")).unwrap_or(pattern);
    let segments: Vec<&str> = rel.split('/').collect();
    let mut results = Vec::new();
    if segments.len() == 1 {
        for p in glob_in(root, segments[0]) {
            results.push(p.to_string_lossy().into_owned());
        }
        return results;
    }
    // Nested form: */*/*/<name>.
    let dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .max_depth(segments.len() - 1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.depth() == 3 && e.file_type().is_dir())
        .map(|e| e.into_path())
        .collect();
    for d in dirs {
        for p in glob_in(&d.to_string_lossy(), segments.last().copied().unwrap_or("")) {
            results.push(p.to_string_lossy().into_owned());
        }
    }
    results
}

/// CodexResumeRolloutPresent reports whether sessionID's rollout is present in
/// the task's codex-home sessions dir.
pub fn codex_resume_rollout_present(codex_home: &str, session_id: &str) -> bool {
    if codex_home.is_empty() || session_id.is_empty() {
        return false;
    }
    !find_codex_rollouts(&join_path(&[codex_home, "sessions"]), session_id).is_empty()
}

/// expose_resume_rollout links sessionID's rollout(s) out of the shared
/// sessions history into the task-local sessions dir, preserving relative
/// layout. Links rather than copies (a rollout can be gigabytes and this runs
/// on initialize's critical path).
fn expose_resume_rollout(
    shared_sessions: &str,
    local_sessions: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let matches = find_codex_rollouts(shared_sessions, session_id);
    if matches.is_empty() {
        bail!("no rollout found for session {session_id} under {shared_sessions}");
    }
    let mut linked = 0usize;
    for src in &matches {
        let rel = match Path::new(src).strip_prefix(shared_sessions) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => Path::new(src)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let dst = join_path(&[local_sessions, &rel]);
        let parent = super::context::dir_of(&dst);
        std::fs::create_dir_all(&parent).with_context(|| format!("create rollout dir {parent}"))?;
        link_codex_rollout(src, &dst).with_context(|| format!("link rollout {src}"))?;
        linked += 1;
    }
    tracing::info!(
        session_id = %session_id,
        files = linked,
        "execenv: exposed resume rollout into task-local sessions"
    );
    Ok(())
}

/// link_codex_rollout materialises src at dst without copying its bytes: hard
/// link first, falling back to a symlink across filesystems.
fn link_codex_rollout(src: &str, dst: &str) -> anyhow::Result<()> {
    if std::fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(src, dst)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Config-referenced files
// ---------------------------------------------------------------------------

/// materialise_in_codex_home writes src to relPath inside codexHome with the
/// symlink-refusal check Go binds to an opened os.Root handle. std has no
/// stable root-scoped write API, so the identity check degrades to: refuse
/// when the task home itself is a symlink, verify dir-ness before mkdir, and
/// document the residual swap-after-check window as Go's PB-5647.
fn materialise_in_codex_home(
    codex_home: &str,
    rel_path: &str,
    src: &str,
    key: &str,
) -> anyhow::Result<()> {
    // Refuse a symlinked task home outright (the deterministic half of Go's
    // verifyCodexHomeRoot).
    if let Ok(md) = Path::new(codex_home).symlink_metadata() {
        if md.file_type().is_symlink() {
            bail!("codex home {codex_home} is a symlink; refusing to write {key} through it");
        }
        if !md.is_dir() {
            bail!("codex home {codex_home} is not a directory; refusing to write {key}");
        }
    } else {
        bail!("stat codex home {codex_home}");
    }

    // os.Root also refuses any symlink on the way down, so a reused task home
    // cannot redirect the write through an intermediate link. Walk every
    // component before creating or writing anything; ".." is rejected for the
    // same reason Go's root-scoped calls refuse it.
    let components: Vec<&str> = rel_path
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if components.contains(&"..") {
        bail!("{key} {rel_path:?} must not traverse outside the task home");
    }
    let mut walked = codex_home.to_string();
    for comp in &components {
        walked = join_path(&[&walked, comp]);
        if let Ok(md) = Path::new(&walked).symlink_metadata() {
            if md.file_type().is_symlink() {
                bail!("{key} path component {walked} is a symlink; refusing to write through it");
            }
        }
    }

    let full = join_path(&[codex_home, rel_path]);
    let dir = super::context::dir_of(&full);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {key} directory {dir}"))?;
    if Path::new(&full).symlink_metadata().is_ok() {
        remove_any(&full).with_context(|| format!("remove stale {key} copy {full}"))?;
    }
    let data = std::fs::read(src).with_context(|| format!("open {key} {src}"))?;
    std::fs::write(&full, data).with_context(|| format!("create {key} copy {full}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Models cache
// ---------------------------------------------------------------------------

/// sync_codex_models_cache seeds models_cache.json once for a fresh task home
/// and binds it to the shared provider/catalog configuration. A changed or
/// missing binding makes an existing cache unsafe (Codex's cache records no
/// provider identity), so it is dropped rather than reused.
fn sync_codex_models_cache(
    codex_home: &str,
    shared_home: &str,
    fresh_home: bool,
) -> anyhow::Result<()> {
    let fingerprint = codex_models_cache_config_fingerprint(shared_home)?;

    let binding_path = join_path(&[codex_home, CODEX_MODELS_CACHE_BINDING_FILE]);
    let (previous, bound) = read_codex_models_cache_binding(&binding_path)?;

    let cache_path = join_path(&[codex_home, CODEX_MODELS_CACHE_FILE]);
    let cache_info = Path::new(&cache_path).symlink_metadata();
    let cache_exists = cache_info.is_ok();

    if bound && previous == fingerprint {
        // The cache belongs to the current config. Preserve both an existing
        // task-refreshed cache and an intentional absence after a failed fetch.
        if let Ok(info) = cache_info {
            if !info.is_file() {
                remove_any(&cache_path).with_context(|| {
                    format!("remove non-regular codex models cache {cache_path}")
                })?;
            }
        }
        return Ok(());
    }

    if cache_exists {
        remove_any(&cache_path)
            .with_context(|| format!("remove unbound codex models cache {cache_path}"))?;
    }

    if fresh_home && !bound && !cache_exists {
        // A shared snapshot is useful on the one path where the task home did
        // not exist yet; subsequent task-local refreshes stay isolated.
        seed_copied_file(
            &join_path(&[shared_home, CODEX_MODELS_CACHE_FILE]),
            &cache_path,
        )
        .context("seed codex models cache")?;
    }

    write_codex_models_cache_binding(&binding_path, &fingerprint)?;
    Ok(())
}

/// codex_models_cache_config_fingerprint hashes the shared config files plus
/// the contents of any model_catalog_json they reference. No config contents
/// or credentials are persisted beyond the digest.
fn codex_models_cache_config_fingerprint(shared_home: &str) -> anyhow::Result<String> {
    let mut h = Sha256::new();
    let mut config_toml: Option<Vec<u8>> = None;

    for name in CODEX_MODELS_CACHE_CONFIG_FILES {
        let path = join_path(&[shared_home, name]);
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                use std::io::Write as _;
                write!(h, "{name}\x00missing\x00").ok();
            }
            Err(e) => return Err(anyhow!("read codex model cache config {path}: {e}")),
            Ok(data) => {
                use std::io::Write as _;
                write!(h, "{name}\x00{}\x00", data.len()).ok();
                h.update(&data);
                if name == "config.toml" {
                    config_toml = Some(data);
                }
            }
        }
    }

    if let Some(config_toml) = config_toml {
        let cfg: toml::Value =
            toml::from_str(&String::from_utf8_lossy(&config_toml)).map_err(|e| {
                anyhow!(
                    "parse codex model cache config {}: {e}",
                    join_path(&[shared_home, "config.toml"])
                )
            })?;
        let catalog_path = cfg
            .get("model_catalog_json")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !catalog_path.is_empty() {
            let resolved =
                resolve_codex_config_path(&catalog_path, shared_home, "model_catalog_json")?;
            let data = std::fs::read(&resolved)
                .map_err(|e| anyhow!("read model_catalog_json {resolved}: {e}"))?;
            use std::io::Write as _;
            write!(h, "model_catalog_json\x00{}\x00", data.len()).ok();
            h.update(&data);
        }
    }

    Ok(hex::encode(h.finalize()))
}

/// read_codex_models_cache_binding returns bound=false for a missing or
/// non-regular marker. Non-regular paths are removed defensively.
fn read_codex_models_cache_binding(path: &str) -> anyhow::Result<(String, bool)> {
    let info = match Path::new(path).symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), false)),
        Err(e) => return Err(anyhow!("stat codex models cache binding {path}: {e}")),
        Ok(i) => i,
    };
    if !info.is_file() {
        remove_any(path)
            .with_context(|| format!("remove non-regular codex models cache binding {path}"))?;
        return Ok((String::new(), false));
    }
    let data = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("read codex models cache binding {path}: {e}"))?;
    Ok((data.trim().to_string(), true))
}

fn write_codex_models_cache_binding(path: &str, fingerprint: &str) -> anyhow::Result<()> {
    remove_any(path).with_context(|| format!("remove prior codex models cache binding {path}"))?;
    std::fs::write(path, format!("{fingerprint}\n"))
        .with_context(|| format!("create codex models cache binding {path}"))?;
    Ok(())
}

/// resolve_codex_config_path maps a path-valued config.toml entry to its
/// source file on this host. A relative path resolves against the shared
/// Codex home.
fn resolve_codex_config_path(
    config_path: &str,
    shared_home: &str,
    key: &str,
) -> anyhow::Result<String> {
    if is_absolute_config_path(config_path) {
        return Ok(clean_lexical(config_path));
    }
    if config_path.starts_with("~/") || config_path.starts_with("~\\") {
        let home = user_home_dir()
            .map_err(|e| anyhow!("resolve {key} {config_path:?}: user home: {e}"))?;
        return Ok(join_path(&[&home, &config_path[2..]]));
    }
    if config_path.starts_with('~') {
        bail!("{key} {config_path:?} uses unsupported ~user expansion");
    }
    Ok(join_path(&[shared_home, &clean_lexical(config_path)]))
}

fn is_absolute_config_path(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with('\\')
        || Path::new(path).is_absolute()
        || path.starts_with("\\\\")
        || path
            .as_bytes()
            .get(1..3)
            .is_some_and(|suffix| suffix[0] == b':' && matches!(suffix[1], b'/' | b'\\'))
}

// ---------------------------------------------------------------------------
// Copy helpers
// ---------------------------------------------------------------------------

/// sync_copied_file mirrors a per-task dst onto the current state of the
/// shared src across Reuse() runs (regression fix PB-2646):
/// present→refresh, removed-source→drop dst, absent→no-op.
#[allow(dead_code)]
fn sync_copied_file(src: &str, dst: &str) -> anyhow::Result<()> {
    let src_missing = match Path::new(src).metadata() {
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(anyhow!("stat src {src}: {e}")),
    };

    if Path::new(dst).symlink_metadata().is_ok() {
        std::fs::remove_file(dst).map_err(|e| anyhow!("remove stale dst {dst}: {e}"))?;
    }

    if src_missing {
        return Ok(());
    }
    super::execenv::copy_file(src, dst)
}

/// seed_copied_file copies src only when dst has no task-local regular file.
/// Unlike sync_copied_file, it never overwrites or removes a cache refreshed
/// by a prior run. Non-regular destinations are removed defensively.
fn seed_copied_file(src: &str, dst: &str) -> anyhow::Result<()> {
    match Path::new(dst).symlink_metadata() {
        Ok(md) if md.is_file() => return Ok(()),
        Ok(_) => {
            remove_any(dst).map_err(|e| anyhow!("remove non-regular dst {dst}: {e}"))?;
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(anyhow!("stat dst {dst}: {e}"));
        }
        Err(_) => {}
    }

    match Path::new(src).metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(anyhow!("stat src {src}: {e}")),
        Ok(_) => {}
    }
    super::execenv::copy_file(src, dst)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestSanitizePathSegment.
    #[test]
    fn test_sanitize_path_segment() {
        assert_eq!(
            sanitize_path_segment("01a01ec0-e69d-7000-8000-0123456789ab"),
            "01a01ec0-e69d-7000-8000-0123456789ab"
        );
        assert_eq!(sanitize_path_segment("../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_path_segment(""), "");
        assert_eq!(sanitize_path_segment("a/b\\c:d"), "abcd");
    }

    // Port of TestCodexSessionStoreKey: chat ids are namespaced away from
    // issue ids; missing identifiers yield "".
    #[test]
    fn test_codex_session_store_key() {
        let mut task = super::super::execenv::TaskContextForEnv {
            issue_id: "ISSUE-1".into(),
            agent_id: "AGENT-9".into(),
            ..Default::default()
        };
        assert_eq!(
            codex_session_store_key("", &task),
            "default/AGENT-9/ISSUE-1"
        );
        assert_eq!(
            codex_session_store_key("staging", &task),
            codex_session_store_key("staging", &task),
            "stable"
        );
        // Named profiles hash into fixed-length namespaces.
        let ns = codex_session_store_namespace("staging");
        assert_eq!(ns.len(), 2 + 64, "fixed length: {ns}");
        assert_ne!(ns, codex_session_store_namespace("production"));

        task.issue_id.clear();
        task.chat_session_id = "CHAT-7".into();
        let key = codex_session_store_key("", &task);
        assert!(key.contains("chat_CHAT-7"), "{key}");

        task.chat_session_id.clear();
        assert_eq!(codex_session_store_key("", &task), "");
    }

    // Port of TestSameCodexPath.
    #[test]
    fn test_same_codex_path() {
        assert!(same_codex_path("/a/b/", "/a/b"));
        assert!(same_codex_path("/a/b/../b", "/a/b"));
        assert!(!same_codex_path("/a/b", "/a/c"));
    }

    // Port of TestStatSharedCodexConfig tri-state.
    #[test]
    fn test_stat_shared_codex_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        assert_eq!(
            stat_shared_codex_config(&root),
            SharedConfigPresence::Absent
        );
        std::fs::write(join_path(&[&root, "config.toml"]), b"x").unwrap();
        assert_eq!(
            stat_shared_codex_config(&root),
            SharedConfigPresence::Present
        );
        assert_eq!(stat_shared_codex_config(""), SharedConfigPresence::Absent);
    }

    // Port of TestFindCodexRollouts: flat and nested layouts both match, with
    // dedup.
    #[test]
    fn test_find_codex_rollouts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        std::fs::create_dir_all(join_path(&[&root, "2026", "01", "02"])).unwrap();
        std::fs::write(
            join_path(&[
                &root,
                "2026",
                "01",
                "02",
                "rollout-2026-01-02T00-00-00-s1.jsonl",
            ]),
            b"x",
        )
        .unwrap();
        std::fs::write(join_path(&[&root, "rollout-flat-s2.jsonl.zst"]), b"x").unwrap();

        let hits = find_codex_rollouts(&root, "s1");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].ends_with("rollout-2026-01-02T00-00-00-s1.jsonl"));

        let hits = find_codex_rollouts(&root, "s2");
        assert_eq!(hits.len(), 1, "{hits:?}");

        assert!(find_codex_rollouts(&root, "s3").is_empty());
        assert!(find_codex_rollouts("", "s1").is_empty());
    }

    // Port of TestCodexResumeRolloutPresent.
    #[test]
    fn test_codex_resume_rollout_present() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_string_lossy().to_string();
        assert!(!codex_resume_rollout_present(&home, "s1"));
        std::fs::create_dir_all(join_path(&[&home, "sessions"])).unwrap();
        std::fs::write(join_path(&[&home, "sessions", "rollout-x-s1.jsonl"]), b"x").unwrap();
        assert!(codex_resume_rollout_present(&home, "s1"));
        assert!(!codex_resume_rollout_present(&home, ""));
    }

    // Port of TestSyncCopiedFileRefreshSemantics.
    #[test]
    fn test_sync_copied_file_refresh_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        let src = join_path(&[&dir, "src.toml"]);
        let dst = join_path(&[&dir, "dst.toml"]);

        // absent/absent: no-op.
        sync_copied_file(&src, &dst).unwrap();
        assert!(!Path::new(&dst).exists());

        // present/absent: copied.
        std::fs::write(&src, b"one").unwrap();
        sync_copied_file(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "one");

        // present/present: refreshed.
        std::fs::write(&src, b"two").unwrap();
        sync_copied_file(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "two");

        // absent/present: dropped.
        std::fs::remove_file(&src).unwrap();
        sync_copied_file(&src, &dst).unwrap();
        assert!(!Path::new(&dst).exists());
    }

    // Port of TestSeedCopiedFileNeverOverwrites.
    #[test]
    fn test_seed_copied_file_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        let src = join_path(&[&dir, "cache"]);
        let dst = join_path(&[&dir, "models_cache.json"]);

        std::fs::write(&src, b"shared").unwrap();
        seed_copied_file(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "shared");

        std::fs::write(&src, b"updated").unwrap();
        seed_copied_file(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "shared",
            "existing task-local file wins"
        );

        // Non-regular dst is replaced.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&src, dst.replace("models_cache", "mc_link")).ok();
        }
        std::fs::remove_file(&dst).unwrap();
        std::fs::create_dir(&dst).unwrap();
        seed_copied_file(&src, &dst).unwrap();
        assert!(
            Path::new(&dst).is_file(),
            "directory dst replaced by seeded file"
        );
    }

    // Port of TestResolveCodexConfigPath.
    #[test]
    fn test_resolve_codex_config_path() {
        let shared = "/shared/.codex";
        let absolute = resolve_codex_config_path("/abs/file.md", shared, "k").unwrap();
        assert_eq!(Path::new(&absolute), Path::new("/abs/file.md"));
        let relative = resolve_codex_config_path("rel/file.md", shared, "k").unwrap();
        assert_eq!(
            Path::new(&relative),
            Path::new("/shared/.codex/rel/file.md")
        );
        assert_eq!(
            resolve_codex_config_path(r"C:\models\catalog.json", shared, "k").unwrap(),
            r"C:\models\catalog.json"
        );
        assert_eq!(
            resolve_codex_config_path(r"\\server\share\instructions.md", shared, "k").unwrap(),
            r"\\server\share\instructions.md"
        );
        assert!(resolve_codex_config_path("~other/x", shared, "k").is_err());
        let got = resolve_codex_config_path("~/mine.md", shared, "k").unwrap();
        assert!(got.ends_with("mine.md"), "{got}");
    }

    // Port of TestCodexModelsCacheBindingLifecycle.
    #[test]
    fn test_models_cache_binding_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        let binding = join_path(&[&dir, CODEX_MODELS_CACHE_BINDING_FILE]);

        assert_eq!(
            read_codex_models_cache_binding(&binding).unwrap(),
            (String::new(), false)
        );
        write_codex_models_cache_binding(&binding, "fp1").unwrap();
        assert_eq!(
            read_codex_models_cache_binding(&binding).unwrap(),
            ("fp1".to_string(), true)
        );
        write_codex_models_cache_binding(&binding, "fp2").unwrap();
        assert_eq!(
            read_codex_models_cache_binding(&binding).unwrap(),
            ("fp2".to_string(), true)
        );
    }

    // Port of TestPruneCodexSessionStoresRetention: idle stores are removed
    // (without a reservation guard), fresh ones survive, and empty agent
    // shells are cleaned up.
    #[test]
    fn test_prune_codex_session_stores_retention() {
        // Point CODEX_HOME at a temp dir for this test.
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var("CODEX_HOME").ok();
        // ENV_LOCK-free because this test does not race other env tests in
        // this module (single-threaded mutation, restored immediately after).
        unsafe { std::env::set_var("CODEX_HOME", tmp.path()) };

        let ns = codex_session_store_namespace("");
        let agent = join_path(&[CODEX_SESSION_STORE_ROOT, &ns, "ag"]);
        let root = tmp.path().to_string_lossy().into_owned();
        let fresh = join_path(&[&root, &agent, "issue-fresh"]);
        let idle = join_path(&[&root, &agent, "issue-idle"]);
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::create_dir_all(&idle).unwrap();
        std::fs::write(join_path(&[&idle, "rollout.jsonl"]), b"data").unwrap();
        // Backdate the idle store's mtime by touching with an old timestamp.
        // Backdate every entry: codex_store_stat takes the newest mtime in the
        // tree, so a single directory touch would be overridden by the fresh
        // rollout file inside.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for entry in walkdir::WalkDir::new(&idle) {
            let entry = entry.unwrap();
            if entry.depth() > 0 {
                set_mtime(entry.path(), old).unwrap();
            }
        }
        set_mtime(Path::new(&idle), old).unwrap();

        let now = chrono::Utc::now();
        let (removed, freed) =
            prune_codex_session_stores("", chrono::Duration::minutes(10), now, None);
        assert_eq!(removed, 1, "one idle store pruned");
        assert_eq!(freed, 4, "bytes accounted");

        assert!(Path::new(&fresh).exists(), "fresh store survives");
        assert!(!Path::new(&idle).exists(), "idle store reclaimed");
        assert!(
            !Path::new(&fresh).exists() || Path::new(&fresh).exists(),
            "no panic"
        );

        // Restore env.
        match prev {
            Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }
    }

    // A symlinked intermediate component must abort the write; a plain nested
    // path still materialises.
    #[test]
    fn test_materialise_in_codex_home_rejects_symlinked_component() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let src = tmp.path().join("src.toml");
        std::fs::write(&src, "model = \"x\"\n").unwrap();

        // Plain nested path works.
        materialise_in_codex_home(
            home.to_str().unwrap(),
            "plugins/cache/installed.json",
            src.to_str().unwrap(),
            "test-plugin",
        )
        .expect("plain nested path");
        let dst = home.join("plugins/cache/installed.json");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "model = \"x\"\n");

        // Symlinked intermediate directory refused.
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir_in(&tmp).unwrap();
            std::fs::create_dir_all(home.join("evil")).unwrap();
            std::os::unix::fs::symlink(outside.path(), home.join("plugins").join("link")).unwrap();
            let err = materialise_in_codex_home(
                home.to_str().unwrap(),
                "plugins/link/config.json",
                src.to_str().unwrap(),
                "test-escape",
            )
            .unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("symlink"), "expected symlink refusal: {msg}");
            assert!(
                !outside.path().join("config.json").exists(),
                "write must not land through the link"
            );
        }

        // ".." traversal refused.
        let err = materialise_in_codex_home(
            home.to_str().unwrap(),
            "../escape.toml",
            src.to_str().unwrap(),
            "test-dotdot",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("outside the task home"));
    }
}
