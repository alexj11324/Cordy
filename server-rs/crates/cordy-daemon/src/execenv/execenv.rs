//! Port of execenv/execenv.go.
//!
//! Symbol map:
//! - RepoContextForEnv            → RepoContextForEnv
//! - ProjectResourceForEnv        → ProjectResourceForEnv (custom Serialize keeps
//!   resource_ref raw, defaulting to {})
//! - PrepareParams                → PrepareParams
//! - TaskContextForEnv            → TaskContextForEnv
//! - SkillContextForEnv           → SkillContextForEnv
//! - SkillFileContextForEnv       → SkillFileContextForEnv
//! - Environment                  → Environment
//! - PredictRootDir               → predict_root_dir
//! - Prepare                      → prepare
//! - ReuseParams / Reuse          → ReuseParams / reuse
//! - hydrateCodexSkills           → hydrate_codex_skills
//! - GCMetaKind / GCMeta          → GCMetaKind / GcMeta
//! - WriteGCMeta / ReadGCMeta     → write_gc_meta / read_gc_meta
//! - ManagedEnvProvenance (+ManagedBy const)
//!   → ManagedEnvProvenance (+ MANAGED_ENV_PROVENANCE_MANAGED_BY)
//! - WriteManagedEnvProvenance /
//!   ReadManagedEnvProvenance     → write_managed_env_provenance / read_managed_env_provenance
//! - Cleanup                      → Environment::cleanup
//! - envRootOwnerFile             → ENV_ROOT_OWNER_FILE
//! - claimEnvRoot                 → claim_env_root
//! - writeEnvRootOwnerExclusive   → write_env_root_owner_exclusive
//! - readEnvRootOwner             → read_env_root_owner
//! - resetEnvRootContents         → reset_env_root_contents
//! - dirIsEmpty                   → dir_is_empty
//!
//! Shared package helpers hosted here:
//! - filepath.Join / filepath.Clean → join_path / clean_path (lexical Go
//!   semantics; std::path does not eliminate `..` the way Go's Clean does)
//! - copyFile (codex_home.go)     → copy_file
//!
//! Deviations:
//! - slog logger parameter dropped; tracing macros used directly.
//! - Prepare is async: the worktree branch shells out to git through
//!   tokio::process with timeouts (local_worktree.rs).
//! - Hermes and OpenClaw remain explicit fail-closed stand-ins at the bottom
//!   of this file. Reasonix and QwenPaw are implemented in their capability
//!   modules and their prepare/reuse call sites are production-wired here.
//! - OpenclawGatewayPin is a structural stand-in for openclaw_config.go's
//!   type. Go's public type masks Token via MarshalJSON/Stringer; the
//!   stand-in serializes plainly (the isolation helper protocol needs the
//!   real token anyway) and masks only in Display. When E2 lands the real
//!   port it replaces this definition wholesale.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::codex_home::{codex_session_store_key, prepare_codex_home_with_opts, CodexHomeOptions};
use super::context::{
    ensure_workspaces_root_marker, prepare_claude_skill_settings, resolve_skill_slugs,
    roll_back_prepared_sidecars, write_context_files, write_skill_files, SidecarManifest,
};
use super::cursor_mcp::prepare_cursor_mcp_config;
use super::git::task_key;
use super::local_worktree::{prepare_local_worktree, LocalWorktree, LocalWorktreeParams};
use super::reasonix;
use super::reclaimable::CODEX_HOME_DIR_NAME;

// ---------------------------------------------------------------------------
// Path helpers (Go filepath.Join / filepath.Clean semantics)
// ---------------------------------------------------------------------------

/// clean_path ports Go's `filepath.Clean` (unix separators): collapses
/// duplicate separators, removes `.` elements, resolves inner `..` lexically,
/// drops leading `..` on rooted paths, and returns "." for an empty result.
pub fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let bytes = path.as_bytes();
    let rooted = bytes[0] == b'/';
    let n = bytes.len();

    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut r = 0usize;
    let mut dotdot = 0usize;
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }

    while r < n {
        if bytes[r] == b'/' {
            // Empty path element.
            r += 1;
        } else if bytes[r] == b'.' && (r + 1 == n || bytes[r + 1] == b'/') {
            // `.` element.
            r += 1;
        } else if bytes[r] == b'.'
            && r + 1 < n
            && bytes[r + 1] == b'.'
            && (r + 2 == n || bytes[r + 2] == b'/')
        {
            // `..` element: remove to last separator.
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            // Real path element; add separator if needed.
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && bytes[r] != b'/' {
                out.push(bytes[r]);
                r += 1;
            }
        }
    }

    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// join_path ports Go's `filepath.Join`: empty elements are ignored, the rest
/// are joined with `/` and the result is Cleaned. Returns "" when every
/// element is empty, matching Go.
pub fn join_path(parts: &[&str]) -> String {
    let mut joined = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !joined.is_empty() {
            joined.push('/');
        }
        joined.push_str(part);
    }
    if joined.is_empty() {
        return String::new();
    }
    clean_path(&joined)
}

/// copy_file copies src to dst unconditionally (Go codex_home.go copyFile).
pub(crate) fn copy_file(src: &str, dst: &str) -> anyhow::Result<()> {
    let data = std::fs::read(src).with_context(|| format!("open {src}"))?;
    // O_EXCL create: callers remove any prior dst first (syncCopiedFile) or
    // probe absence themselves (seedCopiedFile / materialiseInCodexHome).
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .with_context(|| format!("create {dst}"))?;
    f.write_all(&data)
        .with_context(|| format!("copy {src} → {dst}"))?;
    Ok(())
}

/// user_home_dir mirrors os.UserHomeDir for the resolveSharedCodexHome /
/// resolveCodexConfigPath fallbacks.
pub(crate) fn user_home_dir() -> anyhow::Result<String> {
    #[cfg(unix)]
    {
        std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| anyhow!("cannot resolve user home directory"))
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .ok()
            .filter(|h| !h.is_empty())
            .map(String::from)
            .ok_or_else(|| anyhow!("cannot resolve user home directory"))
    }
}

// ---------------------------------------------------------------------------
// Context structs
// ---------------------------------------------------------------------------

/// RepoContextForEnv describes a workspace repo available for checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RepoContextForEnv {
    /// Remote URL.
    pub url: String,
    /// Optional repo description.
    pub description: String,
    /// Optional default checkout ref for this task. Renamed because "ref" is
    /// a Rust keyword; the wire name stays Go's `Ref`.
    #[serde(rename = "Ref")]
    pub reference: String,
}

/// ProjectResourceForEnv describes a single resource attached to the issue's
/// project. The resource_ref payload is type-specific JSON; the agent reads
/// resources.json on disk for the full structure. This struct only carries
/// fields the meta-skill template needs to render a human-readable summary
/// (URL for github_repo, generic label otherwise).
///
/// The custom Serialize mirrors Go's MarshalJSON: resource_ref is emitted as
/// raw JSON (defaulting to `{}` when unset), label carries omitempty, and the
/// field order matches the Go alias struct (id, resource_type, resource_ref,
/// label).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectResourceForEnv {
    /// Server-assigned UUID.
    pub id: String,
    /// e.g. "github_repo".
    pub resource_type: String,
    /// Raw JSONB payload from the API; None renders as `{}` like Go's empty
    /// json.RawMessage.
    pub resource_ref: Option<Value>,
    /// Optional user-supplied label.
    pub label: String,
}

impl Serialize for ProjectResourceForEnv {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("resource_type", &self.resource_type)?;
        let reference = self
            .resource_ref
            .clone()
            .unwrap_or_else(|| Value::Object(Default::default()));
        map.serialize_entry("resource_ref", &reference)?;
        if !self.label.is_empty() {
            map.serialize_entry("label", &self.label)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ProjectResourceForEnv {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            id: String,
            #[serde(default)]
            resource_type: String,
            #[serde(default)]
            resource_ref: Option<Value>,
            #[serde(default)]
            label: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(ProjectResourceForEnv {
            id: raw.id,
            resource_type: raw.resource_type,
            resource_ref: raw.resource_ref,
            label: raw.label,
        })
    }
}

// S9-integration: ThreadReplyTarget lives in reply_instructions.go (lane E3);
// TaskContextForEnv.CommentReplyTargets needs the shape now so the wire
// structs round-trip. Replace with the reply_instructions.rs port when it
// lands — field names must stay ThreadID/ParentID on the wire.
/// One root-thread group a coalesced run must answer (reply_instructions.go).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ThreadReplyTarget {
    /// Root comment id labeling the conversation.
    pub thread_id: String,
    /// The exact `--parent` the agent must pass.
    pub parent_id: String,
}

/// PrepareParams holds all inputs needed to set up an execution environment.
///
/// Serde field names match the Go struct's default JSON names because the
/// isolation helper protocol (isolation.rs) marshals this struct over stdin.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PrepareParams {
    /// Base path for all envs (e.g., ~/cordy_workspaces).
    pub workspaces_root: String,
    /// Workspace UUID — tasks are grouped under this.
    pub workspace_id: String,
    /// Task UUID — used for directory name.
    pub task_id: String,
    /// For git branch naming only.
    pub agent_name: String,
    /// The daemon's profile name (empty = default). Namespaces the per-issue
    /// Codex session store so a second profile-daemon sharing the same
    /// ~/.codex cannot see or GC this daemon's stores (MUL-4424).
    pub profile: String,
    /// Agent provider (determines runtime config and skill injection paths).
    pub provider: String,
    /// Detected Codex CLI version (only used when provider == "codex").
    pub codex_version: String,
    /// Resolved openclaw CLI path (only used when provider == "openclaw");
    /// empty = look up on PATH.
    pub openclaw_bin: String,
    /// The agent's saved `mcp_config` JSON forwarded to provider-specific
    /// config preparers (Cursor/OpenClaw consume it here).
    pub mcp_config: Option<Value>,
    /// Explicit opt-in path to a Cursor mcp-auth.json file or its containing
    /// project data directory. Only Cursor's managed MCP path consumes it.
    pub cursor_mcp_auth_source: String,
    /// Pins the OpenClaw Gateway endpoint inside the per-task wrapper
    /// (issue #3260). Zero means "inherit the user's global config".
    pub openclaw_gateway: OpenclawGatewayPin,
    /// When non-empty, redirects the agent's working directory to a
    /// user-supplied absolute path instead of the synthesised envRoot/workdir
    /// (local_directory flow, MUL-2663). Not copied or mounted.
    pub local_work_dir: String,
    /// Worktree-mode counterpart of local_work_dir: the task gets its own git
    /// worktree of that repo inside envRoot and delivers work as a branch.
    /// Mutually exclusive with local_work_dir.
    pub local_worktree: Option<LocalWorktreeParams>,
    /// Shared Hermes home the per-task overlay is seeded from (hermes only).
    pub hermes_source_home: String,
    /// Fails the overlay build closed when hermes_source_home is absent.
    pub hermes_source_must_exist: bool,
    /// Persistent Hermes memory store the overlay links memories/ to.
    pub hermes_memory_store: String,
    /// Conversation's persistent Hermes session store the overlay links
    /// state.db to.
    pub hermes_session_store: String,
    /// Sanitized effective env used to expand ${VAR} in Hermes external_dirs.
    pub hermes_env: HashMap<String, String>,
    /// Sanitized agent custom_env layered over the daemon's own environment
    /// (reasonix only).
    pub reasonix_env: HashMap<String, String>,
    /// Effective Codex CLI args this task launches with. Only the Windows
    /// sandbox decision reads them (MUL-4957).
    pub codex_custom_args: Vec<String>,
    /// Context data for writing files.
    pub task: TaskContextForEnv,
}

/// TaskContextForEnv is the subset of task context used for writing context
/// files. Serde names match Go's default JSON marshaling (isolation wire).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TaskContextForEnv {
    pub issue_id: String,
    /// Comment that triggered this task (empty for on_assign).
    pub trigger_comment_id: String,
    /// Root comment ID for the triggering thread; falls back to
    /// trigger_comment_id when empty.
    pub trigger_thread_id: String,
    /// Set for a comment run that coalesced comments spanning MORE THAN ONE
    /// root thread (MUL-4348); >=2 entries fan the reply step out per thread.
    pub comment_reply_targets: Vec<ThreadReplyTarget>,
    /// Issue-wide comments since this agent's last run (excludes its own and
    /// the injected trigger).
    pub new_comment_count: i64,
    /// RFC3339 anchor (last run's started_at) the count is measured from;
    /// empty on cold start.
    pub new_comments_since: String,
    /// True when the daemon will resume an existing provider session.
    pub prior_session_resumed: bool,
    /// True when a prior session was expected but could NOT be resumed —
    /// surfaced so the agent tells the user context is gone (MUL-4424).
    pub prior_session_resume_unavailable: bool,
    /// Unique ID of the dispatched agent.
    pub agent_id: String,
    pub agent_name: String,
    /// Agent identity/persona instructions, injected into CLAUDE.md.
    pub agent_instructions: String,
    pub agent_skills: Vec<SkillContextForEnv>,
    pub disabled_runtime_skills: Vec<super::context::RuntimeSkillRefForEnv>,
    /// Workspace repos available for checkout.
    pub repos: Vec<RepoContextForEnv>,
    /// Active project for this task, when present.
    pub project_id: String,
    /// Human-readable project title.
    pub project_title: String,
    /// Durable project-level context rendered into the brief.
    pub project_description: String,
    /// Resources attached to the project.
    pub project_resources: Vec<ProjectResourceForEnv>,
    /// Non-empty for chat tasks.
    pub chat_session_id: String,
    /// IM platform behind a chat session ("slack", "feishu", "wecom"); empty
    /// for web/mobile chat. Names the surface in brief copy (MUL-4899).
    pub chat_channel_type: String,
    /// Server's verdict, for THIS turn, on whether produced files reach the
    /// reader. Carried but deliberately NOT rendered into the brief — it is a
    /// per-turn value and the brief is the prompt-cache prefix (MUL-5377).
    pub chat_channel_delivers_files: bool,

    /// Non-empty for autopilot run_only tasks.
    pub autopilot_run_id: String,
    pub autopilot_id: String,
    pub autopilot_title: String,
    pub autopilot_description: String,
    pub autopilot_source: String,
    pub autopilot_trigger_payload: String,
    /// Non-empty for quick-create tasks.
    pub quick_create_prompt: String,
    /// Assignment handoff instruction; rendered into issue_context.md
    /// (MUL-3375).
    pub handoff_note: String,
    /// True when THIS TASK runs the squad-leader role; derived from the
    /// claim's is_leader_task / squad_id, never sniffed from instruction text
    /// (MUL-5811).
    pub is_squad_leader: bool,
    /// Workspace-level system prompt (workspace.context in the DB), rendered
    /// into the brief as `## Workspace Context` when non-empty.
    pub workspace_context: String,
    /// Per-run external app capabilities mounted through MCP overlays.
    pub connected_apps: Vec<ConnectedApp>,
    /// The human the agent acts on behalf of (runtime owner in v1). Rendered
    /// as `## Requesting User` only when the description is non-empty.
    pub requesting_user_name: String,
    pub requesting_user_profile_description: String,
    /// Initiator* identify the actor who triggered THIS task (MUL-2645);
    /// rendered as `## Task Initiator` when a name is present.
    pub initiator_type: String,
    pub initiator_id: String,
    pub initiator_name: String,
    pub initiator_email: String,
}

/// SkillContextForEnv represents a skill to be written into the execution
/// environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SkillContextForEnv {
    pub name: String,
    pub description: String,
    pub content: String,
    pub files: Vec<SkillFileContextForEnv>,
}

/// SkillFileContextForEnv represents a supporting file within a skill.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SkillFileContextForEnv {
    pub path: String,
    pub content: String,
}

// S9-integration: ConnectedApp lives in internal/runtimeapps (ported with the
// service layer elsewhere); TaskContextForEnv.ConnectedApps needs the wire
// shape now. Field names mirror the Go json tags byte-for-byte.
/// Per-run external app capability (internal/runtimeapps/connected_app.go).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectedApp {
    pub provider: String,
    pub server_name: String,
    pub toolkit_slug: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub toolkit_name: String,
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// Environment represents a prepared, isolated execution environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Environment {
    /// Top-level env directory ({workspaces_root}/{task_id_short}/).
    pub root_dir: String,
    /// Directory passed as Cwd to the agent. Normally {root_dir}/workdir/;
    /// the local_directory flow substitutes the user's path.
    pub work_dir: String,
    /// True when work_dir points at a user-supplied path outside root_dir.
    /// Callers keying on "may I remove work_dir as scratch?" must check this.
    /// Deliberately FALSE in worktree mode (the worktree is disposable).
    pub local_directory: bool,
    /// Private per-task config directory exported to child CLI invocations.
    pub cordy_config_root: String,
    /// Set when the task runs in worktree mode against a local_directory
    /// resource; Finalize commits leftovers and reports the branch.
    pub local_worktree: Option<LocalWorktree>,
    /// Per-task CODEX_HOME directory (set only for codex provider).
    pub codex_home: String,
    /// Task-local --settings JSON applying disabled runtime-skill policy.
    pub claude_settings_path: String,
    /// Per-task synthesized OpenClaw config path (openclaw provider).
    pub openclaw_config_path: String,
    /// Directory of the user's active OpenClaw config (openclaw provider with
    /// an on-disk user config); prepended to OPENCLAW_INCLUDE_ROOTS.
    pub openclaw_include_root: String,
    /// Per-task Cursor data directory (cursor provider with managed
    /// mcp_config); exported as CURSOR_DATA_DIR.
    pub cursor_data_dir: String,
    /// Per-task HERMES_HOME overlay (hermes provider with bound skills).
    pub hermes_home: String,
    /// Session store this task's state.db is actually linked to, or "" when
    /// the session database stayed task-local.
    pub hermes_session_store: String,
    /// Reports that the mounted store actually holds a session database — a
    /// prior turn's transcript this task can resume.
    pub hermes_session_history_present: bool,
    /// Per-task QwenPaw workspace directory (qwenpaw provider).
    pub qwenpaw_workspace: String,
}

/// PredictRootDir returns the env root path that prepare would create for the
/// given task, without performing any I/O. Callers use this to claim ownership
/// of the directory (e.g. against the GC loop) before Prepare/Reuse runs.
pub fn predict_root_dir(workspaces_root: &str, workspace_id: &str, task_id: &str) -> String {
    if workspaces_root.is_empty() || workspace_id.is_empty() || task_id.is_empty() {
        return String::new();
    }
    join_path(&[workspaces_root, workspace_id, &task_key(task_id)])
}

// ---------------------------------------------------------------------------
// Prepare
// ---------------------------------------------------------------------------

/// Prepare creates an isolated execution environment for a task.
/// The workdir starts empty (no repo checkouts). The agent checks out repos
/// on demand via `cordy repo checkout <url>`.
pub async fn prepare(params: PrepareParams) -> anyhow::Result<Environment> {
    if params.workspaces_root.is_empty() {
        bail!("execenv: workspaces root is required");
    }
    if params.workspace_id.is_empty() {
        bail!("execenv: workspace ID is required");
    }
    if params.task_id.is_empty() {
        bail!("execenv: task ID is required");
    }

    let env_root = predict_root_dir(
        &params.workspaces_root,
        &params.workspace_id,
        &params.task_id,
    );

    // Self-heal the root-level daemon marker on every task start so a marker
    // removed while the daemon runs is restored before the agent spawns.
    // Non-fatal: without it the workdir marker still protects the common case.
    if let Err(err) = ensure_workspaces_root_marker(&params.workspaces_root) {
        tracing::warn!(
            error = %format!("{err:#}"),
            "execenv: workspaces root marker not written; fail-closed guard limited to the task workdir"
        );
    }

    // Take exclusive ownership of the env root before touching anything in it
    // (#7326). claim_env_root is atomic end to end: a read-then-delete would
    // let two same-key tasks both pass the check and one still delete the
    // other.
    let fresh =
        claim_env_root(&env_root, &params.task_id).map_err(|e| anyhow!("execenv: {e:#}"))?;
    // Not fresh means this task already owned the directory — a rerun, which
    // is meant to start from a clean tree.
    if !fresh {
        reset_env_root_contents(&env_root)
            .map_err(|e| anyhow!("execenv: reset existing env: {e:#}"))?;
    }

    // Create directory tree. For the standard flow the agent's workdir is
    // envRoot/workdir; for local_directory tasks the user's path takes its
    // place and we only need to create the scratch directories under envRoot.
    let mut work_dir = join_path(&[&env_root, "workdir"]);
    let mut scratch_dirs = vec![
        join_path(&[&env_root, "output"]),
        join_path(&[&env_root, "logs"]),
    ];
    if params.local_work_dir.is_empty() && params.local_worktree.is_none() {
        scratch_dirs.push(work_dir.clone());
    } else if !params.local_work_dir.is_empty() {
        work_dir = params.local_work_dir.clone();
    }
    for dir in &scratch_dirs {
        std::fs::create_dir_all(dir).with_context(|| format!("execenv: create directory {dir}"))?;
    }
    let cordy_config_root = join_path(&[&env_root, "cordy-config"]);
    std::fs::create_dir_all(&cordy_config_root)
        .context("execenv: create task-local Cordy config directory")?;
    restrict_permissions(&cordy_config_root)
        .context("execenv: restrict task-local Cordy config directory")?;

    // Rollback state: everything after worktree creation can still fail, and
    // on those paths the caller never receives an Environment, so nothing
    // downstream knows a worktree exists to clean up. The manifest rollback is
    // armed before the first context write for the same reason (MUL-6132).
    let mut local_worktree: Option<LocalWorktree> = None;
    let mut manifest = SidecarManifest::default();

    let result = prepare_body(
        &params,
        &env_root,
        &mut work_dir,
        &mut local_worktree,
        &mut manifest,
    )
    .await;

    match result {
        Ok(mut env) => {
            tracing::info!(
                root = %env_root,
                repos_available = params.task.repos.len(),
                "execenv: prepared env"
            );
            env.root_dir = env_root;
            Ok(env)
        }
        Err(err) => {
            // Safe to discard unconditionally: no agent has run yet, so the
            // worktree holds only what Prepare itself put there.
            if let Some(wt) = &local_worktree {
                wt.discard().await;
            }
            // In place only: worktree mode discards the whole worktree above,
            // and a cloud envRoot is wiped wholesale by the GC — only the
            // local_directory flow writes into a directory that outlives the
            // task and belongs to the user.
            if !params.local_work_dir.is_empty() {
                if let Err(rb_err) = roll_back_prepared_sidecars(&manifest) {
                    tracing::warn!(
                        work_dir = %work_dir,
                        error = %format!("{rb_err:#}"),
                        "execenv: roll back sidecars after failed prepare"
                    );
                }
            }
            Err(err)
        }
    }
}

/// prepare_body runs the mutable tail of prepare (worktree creation through
/// manifest persistence) so the caller can run the rollback defers on error.
async fn prepare_body(
    params: &PrepareParams,
    env_root: &str,
    work_dir: &mut String,
    local_worktree: &mut Option<LocalWorktree>,
    manifest: &mut SidecarManifest,
) -> anyhow::Result<Environment> {
    let mut env = Environment {
        root_dir: env_root.to_string(),
        work_dir: work_dir.clone(),
        local_directory: !params.local_work_dir.is_empty(),
        cordy_config_root: join_path(&[env_root, "cordy-config"]),
        ..Default::default()
    };

    // Worktree mode: build the task's own checkout of the user's repo inside
    // envRoot and use it as the workdir. Done before any context file is
    // written so the sidecars land inside the disposable worktree instead of
    // the user's directory.
    if let Some(wt_params) = &params.local_worktree {
        let mut wt_params = wt_params.clone();
        wt_params.env_root = env_root.to_string();
        wt_params.agent_name = params.agent_name.clone();
        wt_params.task_id = params.task_id.clone();
        let wt = prepare_local_worktree(wt_params).await?;
        *work_dir = wt.work_dir.clone();
        // The resource may point at a subdirectory that holds only ignored
        // files, in which case git doesn't materialise it in the worktree.
        std::fs::create_dir_all(&wt.work_dir)
            .with_context(|| format!("execenv: create worktree workdir {}", wt.work_dir))?;
        *local_worktree = Some(wt.clone());
        env.local_worktree = Some(wt);
    }

    // Write context files into workdir (skills go to provider-native paths).
    write_context_files(
        work_dir,
        &params.provider,
        &params.task,
        Some(&mut *manifest),
    )
    .map_err(|e| anyhow!("execenv: write context files: {e:#}"))?;

    // Persist managed-env provenance for non-local resumable envs at Prepare
    // time (not on completion, where .gc_meta.json is written) — MUL-4886.
    // Non-fatal: a write failure only costs the next follow-up its session
    // reuse.
    if params.local_work_dir.is_empty()
        && (!params.task.issue_id.is_empty() || !params.task.chat_session_id.is_empty())
    {
        if let Err(err) = write_managed_env_provenance(
            env_root,
            ManagedEnvProvenance {
                workspace_id: params.workspace_id.clone(),
                issue_id: params.task.issue_id.clone(),
                chat_session_id: params.task.chat_session_id.clone(),
                agent_id: params.task.agent_id.clone(),
                managed_by: String::new(),
            },
        ) {
            tracing::warn!(
                error = %format!("{err:#}"),
                "execenv: write managed env provenance failed (non-fatal); a follow-up may start a fresh session"
            );
        }
    }

    // For Codex, set up a per-task CODEX_HOME seeded from ~/.codex/ with skills.
    if params.provider == "codex" {
        let codex_home = join_path(&[env_root, CODEX_HOME_DIR_NAME]);
        prepare_codex_home_with_opts(
            &codex_home,
            CodexHomeOptions {
                codex_version: params.codex_version.clone(),
                is_local_directory: !params.local_work_dir.is_empty()
                    || params.local_worktree.is_some(),
                session_store_key: codex_session_store_key(&params.profile, &params.task),
                codex_custom_args: params.codex_custom_args.clone(),
                ..Default::default()
            },
        )
        .map_err(|e| anyhow!("execenv: prepare codex-home: {e:#}"))?;
        hydrate_codex_skills(
            &codex_home,
            &params.task.agent_skills,
            &params.task.disabled_runtime_skills,
        )
        .map_err(|e| anyhow!("execenv: hydrate codex skills: {e:#}"))?;
        env.codex_home = codex_home;
    }

    if params.provider == "claude" {
        let settings_path = prepare_claude_skill_settings(
            env_root,
            &params.task.disabled_runtime_skills,
            &params.task.agent_skills,
        )
        .map_err(|e| anyhow!("execenv: prepare claude skill settings: {e:#}"))?;
        env.claude_settings_path = settings_path;
    }

    // For Hermes, redirect HERMES_HOME to a per-task compatibility overlay
    // ONLY when the agent has skills bound (issue #5242). See hermes_home.go.
    //
    // Note this is a local contract, not an observable product behaviour: the
    // server appends built-in skills to every agent's skill set, so the
    // skill-less branch is effectively unreachable in production.
    if params.provider == "hermes" && !params.task.agent_skills.is_empty() {
        let hermes_home = join_path(&[env_root, "hermes-home"]);
        let sessions = prepare_hermes_home(
            &hermes_home,
            &params.hermes_source_home,
            params.hermes_source_must_exist,
            &params.task.agent_skills,
            &params.hermes_env,
            &params.hermes_memory_store,
            &params.hermes_session_store,
        )
        .map_err(|e| anyhow!("execenv: prepare hermes-home: {e:#}"))?;
        env.hermes_home = hermes_home;
        if sessions.mounted {
            env.hermes_session_store = params.hermes_session_store.clone();
            env.hermes_session_history_present = sessions.history_present;
        }
    }
    if params.provider == "qwenpaw" {
        let qwenpaw_workspace = join_path(&[env_root, "qwenpaw-workspace"]);
        prepare_qwenpaw_workspace(&qwenpaw_workspace, &params.task.agent_skills)
            .map_err(|e| anyhow!("execenv: prepare qwenpaw workspace: {e:#}"))?;
        env.qwenpaw_workspace = qwenpaw_workspace;
    }

    // For Reasonix, deny the `ask` tool for this task through a project-scoped
    // reasonix.toml. Degraded, not fatal: without it the task still runs under
    // the backend's fail-closed question handling.
    if params.provider == "reasonix" {
        if let Err(err) =
            write_reasonix_project_config(work_dir, &params.reasonix_env, Some(&mut *manifest))
        {
            tracing::warn!(error = %format!("{err:#}"), "execenv: write reasonix project config failed");
        }
    }

    // For Cursor, materialize managed MCP into project-local config and use
    // an isolated CURSOR_DATA_DIR for the per-workdir approval sidecar.
    if params.provider == "cursor" {
        let cursor_data_dir = prepare_cursor_mcp_config(
            env_root,
            work_dir,
            params.mcp_config.as_ref(),
            &params.cursor_mcp_auth_source,
            Some(&mut *manifest),
        )
        .map_err(|e| anyhow!("execenv: prepare cursor mcp config: {e:#}"))?;
        env.cursor_data_dir = cursor_data_dir;
    }

    // Persist the sidecar manifest. In place the manifest is the ONLY record
    // of what we wrote into the user's own directory, so losing it strands
    // the sidecar tree there permanently (MUL-6132) — fail so the rollback
    // registered by prepare removes the tree now. Elsewhere the manifest is a
    // convenience the GC can do without, so a warning stays the right
    // response.
    if let Err(err) = super::context::write_sidecar_manifest(env_root, manifest) {
        if !params.local_work_dir.is_empty() {
            return Err(anyhow!("execenv: write sidecar manifest: {err:#}"));
        }
        tracing::warn!(error = %format!("{err:#}"), "execenv: write sidecar manifest failed (non-fatal)");
    }

    // For OpenClaw, synthesize a per-task config that pins workspace to
    // workDir. Fail closed on errors: silently degrading to a minimal config
    // would mask a malformed user config.
    if params.provider == "openclaw" {
        let result = prepare_openclaw_config(
            env_root,
            work_dir,
            &OpenclawConfigPrep {
                openclaw_bin: params.openclaw_bin.clone(),
                mcp_config: params.mcp_config.clone(),
                gateway: params.openclaw_gateway.clone(),
            },
        )
        .map_err(|e| anyhow!("execenv: prepare openclaw config: {e:#}"))?;
        env.openclaw_config_path = result.config_path;
        env.openclaw_include_root = result.include_root;
    }

    Ok(env)
}

/// ReuseParams describes the inputs to reuse. It mirrors PrepareParams for
/// the per-provider knobs so callers can pass the same resolved binary path on
/// both first-run and reuse paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ReuseParams {
    /// Daemon-owned root under which all task envs live. Passed on reuse so
    /// the root-level fail-closed marker is self-healed here too.
    pub workspaces_root: String,
    pub work_dir: String,
    pub provider: String,
    /// Only used when provider == "codex".
    pub codex_version: String,
    /// Prior Codex thread/session ID this reused task intends to resume;
    /// consulted while migrating a legacy per-task home whose sessions/ still
    /// symlinks the shared ~/.codex/sessions (MUL-4424).
    pub resume_session_id: String,
    /// Only used when provider == "openclaw"; empty = PATH lookup.
    pub openclaw_bin: String,
    /// Agent's saved `mcp_config` JSON re-materialised into the wrapper.
    pub mcp_config: Option<Value>,
    /// Mirrors PrepareParams.cursor_mcp_auth_source on reuse.
    pub cursor_mcp_auth_source: String,
    /// Per-task Gateway pin re-applied on reuse.
    pub openclaw_gateway: OpenclawGatewayPin,
    /// Profile name mirroring PrepareParams.profile (MUL-4424).
    pub profile: String,
    /// True when the reused work_dir is a user-supplied directory; propagated
    /// into the returned Environment so downstream callers keep the "never
    /// delete the user's directory" invariant on reuse paths.
    pub local_directory: bool,
    /// Hermes mirrors of PrepareParams on reuse.
    pub hermes_source_home: String,
    pub hermes_source_must_exist: bool,
    pub hermes_env: HashMap<String, String>,
    pub hermes_memory_store: String,
    pub hermes_session_store: String,
    /// Reasonix mirror of PrepareParams.reasonix_env on reuse.
    pub reasonix_env: HashMap<String, String>,
    /// Windows sandbox decision input mirrored on reuse (MUL-4957).
    pub codex_custom_args: Vec<String>,
    /// Refreshed context files / skills.
    pub task: TaskContextForEnv,
}

/// Reuse wraps an existing workdir into an Environment and refreshes context
/// files. Returns None if the workdir does not exist (caller should fall back
/// to prepare) or when a refresh failure forces a fresh prepare.
///
/// Sync like Go: no git subprocesses run on this path.
pub fn reuse(params: ReuseParams) -> Option<Environment> {
    if let Err(_e) = std::fs::metadata(&params.work_dir) {
        return None;
    }

    // Self-heal the root-level daemon marker on the reuse path too. Non-fatal:
    // the per-workdir marker still protects the common case, and an empty
    // workspaces_root (legacy callers) simply skips this.
    if !params.workspaces_root.is_empty() {
        if let Err(err) = ensure_workspaces_root_marker(&params.workspaces_root) {
            tracing::warn!(
                error = %format!("{err:#}"),
                "execenv: workspaces root marker not written on reuse; fail-closed guard limited to the task workdir"
            );
        }
    }

    let mut root_dir = super::context::dir_of(&params.work_dir);
    if params.local_directory {
        // For local_directory tasks the user's work_dir is unrelated to
        // envRoot, so reading it from dir_of(work_dir) would point at the
        // parent of the user's directory. v1 only ever reuses local_directory
        // workdirs after a fresh Prepare in the same task lifetime, so the
        // empty root_dir on reuse is fine for current callers.
        root_dir = String::new();
    }
    let mut env = Environment {
        root_dir: root_dir.clone(),
        work_dir: params.work_dir.clone(),
        local_directory: params.local_directory,
        ..Default::default()
    };
    if !env.root_dir.is_empty() {
        env.cordy_config_root = join_path(&[&env.root_dir, "cordy-config"]);
        if let Err(err) = std::fs::create_dir_all(&env.cordy_config_root) {
            tracing::warn!(
                error = %format!("{err:#}"),
                "execenv: restore task-local Cordy config directory failed; forcing fresh prepare"
            );
            return None;
        }
        if restrict_permissions(&env.cordy_config_root).is_err() {
            tracing::warn!(
                "execenv: restrict task-local Cordy config directory failed; forcing fresh prepare"
            );
            return None;
        }
    }

    // Roll back the previous dispatch's sidecar writes before refreshing:
    // without clearing them first, write_skill_files sees its own earlier
    // output occupying the canonical slug and falls back to a collision-free
    // sibling, accumulating a fresh duplicate on every re-dispatch (#3684).
    //
    // Two steps, in order: remove_reused_managed_skill_dirs reclaims the
    // platform's own skill directories even when a prior-run agent left a file
    // inside one; cleanup_sidecars rolls back the remaining sidecar files and
    // the manifest itself. No-op when root_dir is empty or no prior manifest
    // exists.
    if !env.root_dir.is_empty() {
        if let Err(err) = super::context::remove_reused_managed_skill_dirs(
            &env.root_dir,
            &super::context::skills_dir_path(&params.work_dir, &params.provider),
        ) {
            tracing::warn!(error = %format!("{err:#}"), "execenv: reclaim managed skill dirs on reuse failed");
        }
        if let Err(err) = super::context::cleanup_sidecars(&env.root_dir) {
            tracing::warn!(error = %format!("{err:#}"), "execenv: roll back prior sidecars on reuse failed");
            // A failed rollback leaves the previous task's managed files in
            // place. Do not let the refresh path mistake one of those files
            // for a repository-owned collision (especially reasonix.toml),
            // because its stale policy would override the current user
            // configuration. The caller will fall back to a fresh prepare.
            return None;
        }
    }

    // Refresh context files (issue_context.md, skills), tracking a fresh
    // manifest under env.root_dir so a later cleanup sees the up-to-date list
    // of writes.
    let mut manifest = SidecarManifest::default();
    if let Err(err) = write_context_files(
        &params.work_dir,
        &params.provider,
        &params.task,
        Some(&mut manifest),
    ) {
        tracing::warn!(error = %format!("{err:#}"), "execenv: refresh context files failed");
    }

    // Restore CodexHome for Codex provider — re-run prepare_codex_home_with_opts
    // to ensure config (especially sandbox/network access) is up to date.
    if params.provider == "codex" {
        let codex_home = join_path(&[&env.root_dir, CODEX_HOME_DIR_NAME]);
        match prepare_codex_home_with_opts(
            &codex_home,
            CodexHomeOptions {
                codex_version: params.codex_version.clone(),
                resume_session_id: params.resume_session_id.clone(),
                is_local_directory: params.local_directory,
                session_store_key: codex_session_store_key(&params.profile, &params.task),
                codex_custom_args: params.codex_custom_args.clone(),
                ..Default::default()
            },
        ) {
            Ok(()) => {
                if let Err(err) = hydrate_codex_skills(
                    &codex_home,
                    &params.task.agent_skills,
                    &params.task.disabled_runtime_skills,
                ) {
                    tracing::warn!(error = %format!("{err:#}"), "execenv: refresh codex skills failed");
                }
                env.codex_home = codex_home;
            }
            Err(err) => {
                tracing::warn!(error = %format!("{err:#}"), "execenv: refresh codex-home failed");
            }
        }
    }

    if params.provider == "claude" && !env.root_dir.is_empty() {
        match prepare_claude_skill_settings(
            &env.root_dir,
            &params.task.disabled_runtime_skills,
            &params.task.agent_skills,
        ) {
            Ok(settings_path) => env.claude_settings_path = settings_path,
            Err(err) => {
                tracing::warn!(error = %format!("{err:#}"), "execenv: refresh claude skill settings failed");
            }
        }
    }

    // Re-deny Reasonix's `ask` tool on reuse: cleanup_sidecars above removed
    // the prior run's reasonix.toml.
    if params.provider == "reasonix" {
        if let Err(err) = write_reasonix_project_config(
            &params.work_dir,
            &params.reasonix_env,
            Some(&mut manifest),
        ) {
            tracing::warn!(error = %format!("{err:#}"), "execenv: refresh reasonix project config failed");
        }
    }

    // Refresh (or tear down) the per-task QwenPaw workspace on reuse.
    if params.provider == "qwenpaw" && !env.root_dir.is_empty() {
        let qwenpaw_workspace = join_path(&[&env.root_dir, "qwenpaw-workspace"]);
        if let Err(err) = prepare_qwenpaw_workspace(&qwenpaw_workspace, &params.task.agent_skills) {
            tracing::warn!(
                error = %format!("{err:#}"),
                "execenv: refresh qwenpaw workspace failed; forcing fresh prepare"
            );
            return None;
        }
        env.qwenpaw_workspace = qwenpaw_workspace;
    }

    // Refresh (or tear down) the per-task HERMES_HOME on reuse. With skills
    // bound, rebuild the overlay; with none, drop the redirect entirely so the
    // task reverts to the user's real home.
    if params.provider == "hermes" && !env.root_dir.is_empty() {
        let hermes_home = join_path(&[&env.root_dir, "hermes-home"]);
        if !params.task.agent_skills.is_empty() {
            match prepare_hermes_home(
                &hermes_home,
                &params.hermes_source_home,
                params.hermes_source_must_exist,
                &params.task.agent_skills,
                &params.hermes_env,
                &params.hermes_memory_store,
                &params.hermes_session_store,
            ) {
                Ok(sessions) => {
                    env.hermes_home = hermes_home;
                    env.hermes_session_store = String::new();
                    env.hermes_session_history_present = false;
                    if sessions.mounted {
                        env.hermes_session_store = params.hermes_session_store.clone();
                        env.hermes_session_history_present = sessions.history_present;
                    }
                }
                Err(err) => {
                    // Fail closed: a half-built overlay must not run. None
                    // makes the daemon fall back to a fresh prepare.
                    tracing::warn!(
                        error = %format!("{err:#}"),
                        "execenv: refresh hermes-home failed; forcing fresh prepare"
                    );
                    return None;
                }
            }
        } else {
            env.hermes_home = String::new();
            env.hermes_session_store = String::new();
            env.hermes_session_history_present = false;
            if let Err(err) = remove_tree(&hermes_home) {
                tracing::warn!(error = %format!("{err:#}"), "execenv: remove stale hermes-home failed");
            }
        }
    }

    // Refresh Cursor's managed MCP sidecars on reuse.
    if params.provider == "cursor" && !env.root_dir.is_empty() {
        match prepare_cursor_mcp_config(
            &env.root_dir,
            &params.work_dir,
            params.mcp_config.as_ref(),
            &params.cursor_mcp_auth_source,
            Some(&mut manifest),
        ) {
            Ok(cursor_data_dir) => env.cursor_data_dir = cursor_data_dir,
            Err(err) => {
                tracing::warn!(
                    error = %format!("{err:#}"),
                    "execenv: refresh cursor mcp config failed"
                );
                return None;
            }
        }
    }

    if !env.root_dir.is_empty() {
        if let Err(err) = super::context::write_sidecar_manifest(&env.root_dir, &manifest) {
            tracing::warn!(error = %format!("{err:#}"), "execenv: refresh sidecar manifest failed");
        }
    }

    // Refresh the per-task OpenClaw config on reuse. Fail closed.
    if params.provider == "openclaw" {
        match prepare_openclaw_config(
            &env.root_dir,
            &params.work_dir,
            &OpenclawConfigPrep {
                openclaw_bin: params.openclaw_bin.clone(),
                mcp_config: params.mcp_config.clone(),
                gateway: params.openclaw_gateway.clone(),
            },
        ) {
            Ok(result) => {
                env.openclaw_config_path = result.config_path;
                env.openclaw_include_root = result.include_root;
            }
            Err(err) => {
                tracing::warn!(
                    error = %format!("{err:#}"),
                    "execenv: refresh openclaw config failed"
                );
                return None;
            }
        }
    }

    tracing::info!(workdir = %params.work_dir, "execenv: reusing env");
    Some(env)
}

/// hydrateCodexSkills populates the per-task CODEX_HOME/skills directory with
/// both user-installed skills (from the shared ~/.codex/skills/) and
/// workspace-assigned skills. Workspace skills win on name conflict.
///
/// The skills directory is wiped first so stale user-seeded copies and
/// removed user skills cannot linger (see execenv.go for the full rationale).
/// Codex is the only runtime that needs this two-stage hydration because the
/// daemon sets CODEX_HOME to a per-task directory.
pub(crate) fn hydrate_codex_skills(
    codex_home: &str,
    workspace_skills: &[SkillContextForEnv],
    disabled_runtime_skills: &[super::context::RuntimeSkillRefForEnv],
) -> anyhow::Result<()> {
    let skills_dir = join_path(&[codex_home, "skills"]);
    remove_tree(&skills_dir).context("clear codex skills dir")?;
    if let Err(err) = super::codex_user_skills::seed_user_codex_skills(codex_home, workspace_skills)
    {
        tracing::warn!(error = %format!("{err:#}"), "execenv: seed user codex skills failed");
    }
    if !workspace_skills.is_empty() {
        super::context::write_skill_files(&skills_dir, workspace_skills, None)?;
    }
    super::context::ensure_codex_disabled_skills_config(
        &join_path(&[codex_home, "config.toml"]),
        codex_home,
        disabled_runtime_skills,
        workspace_skills,
    )
}

// ---------------------------------------------------------------------------
// GC metadata
// ---------------------------------------------------------------------------

/// GCMetaKind identifies which kind of parent record a task workdir belongs
/// to. The GC loop dispatches its decision tree on this value so chat /
/// autopilot / quick-create tasks are no longer forced through the
/// issue-centric path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GCMetaKind {
    #[serde(rename = "issue")]
    Issue,
    #[serde(rename = "chat")]
    Chat,
    #[serde(rename = "autopilot_run")]
    AutopilotRun,
    #[serde(rename = "quick_create")]
    QuickCreate,
}

/// GCMeta is persisted to .gc_meta.json inside the env root so the GC loop
/// can decide whether the directory is reclaimable. It is a discriminated
/// union keyed on Kind: only the ID field matching Kind is meaningful.
///
/// Older meta files (pre-v2) lack the Kind field; readers must default empty
/// Kind to Issue for backward compatibility.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GcMeta {
    #[serde(rename = "kind", skip_serializing_if = "Option::is_none")]
    pub kind: Option<GCMetaKind>,
    #[serde(rename = "issue_id", skip_serializing_if = "String::is_empty")]
    pub issue_id: String,
    #[serde(rename = "chat_session_id", skip_serializing_if = "String::is_empty")]
    pub chat_session_id: String,
    #[serde(rename = "autopilot_run_id", skip_serializing_if = "String::is_empty")]
    pub autopilot_run_id: String,
    #[serde(rename = "task_id", skip_serializing_if = "String::is_empty")]
    pub task_id: String,
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "completed_at")]
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Marks tasks whose WorkDir pointed at a user-owned path rather than the
    /// synthesised envRoot/workdir. The GC loop honours this by never falling
    /// into the clean branch; pattern-based artifact cleanup is still allowed.
    #[serde(rename = "local_directory", skip_serializing_if = "std::ops::Not::not")]
    pub local_directory: bool,
}

const GC_META_FILE: &str = ".gc_meta.json";

/// WriteGCMeta writes GC metadata into the given directory. The caller is
/// responsible for choosing Kind and populating the matching ID field;
/// CompletedAt is stamped here so callers don't have to think about clocks.
pub fn write_gc_meta(env_root: &str, mut meta: GcMeta) -> anyhow::Result<()> {
    if env_root.is_empty() {
        return Ok(());
    }
    if meta.kind.is_none() {
        // Defensive: a task that doesn't fit any known kind would write a
        // meta file the GC loop can't dispatch on. Skip silently — the
        // directory falls back to the orphan-by-mtime path.
        tracing::debug!(env_root = %env_root, "execenv: skipping .gc_meta.json write: kind is empty");
        return Ok(());
    }
    meta.completed_at = Some(chrono::Utc::now());
    let data = serde_json::to_vec(&meta).context("marshal gc meta")?;
    std::fs::write(Path::new(env_root).join(GC_META_FILE), data)?;
    Ok(())
}

/// ReadGCMeta reads GC metadata from a task directory root. Pre-v2 meta files
/// (no kind field) are normalized to Issue so the legacy issue path keeps
/// working without a migration.
pub fn read_gc_meta(env_root: &str) -> anyhow::Result<GcMeta> {
    let data = std::fs::read(Path::new(env_root).join(GC_META_FILE))?;
    let mut meta: GcMeta = serde_json::from_slice(&data)?;
    if meta.kind.is_none() {
        meta.kind = Some(GCMetaKind::Issue);
    }
    Ok(meta)
}

// ---------------------------------------------------------------------------
// Managed-env provenance
// ---------------------------------------------------------------------------

const MANAGED_ENV_PROVENANCE_FILE: &str = ".managed_env.json";

/// ManagedEnvProvenanceManagedBy discriminates a managed-env provenance file
/// the daemon wrote from any lookalike JSON that happens to share the path.
pub const MANAGED_ENV_PROVENANCE_MANAGED_BY: &str = "cordy-daemon-managed-env";

/// ManagedEnvProvenance is persisted to .managed_env.json inside the env root
/// at Prepare time (NOT on completion, unlike .gc_meta.json). Its whole reason
/// to exist is timing: a squad-leader follow-up can be claimed the instant the
/// prior task completes — before the prior handler writes .gc_meta.json
/// (MUL-4886). Written only for non-local managed issue or chat envs, so its
/// presence is itself the "safe to reuse, not a user local_directory"
/// assertion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManagedEnvProvenance {
    #[serde(rename = "managed_by")]
    pub managed_by: String,
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "issue_id", skip_serializing_if = "String::is_empty")]
    pub issue_id: String,
    #[serde(rename = "chat_session_id", skip_serializing_if = "String::is_empty")]
    pub chat_session_id: String,
    #[serde(rename = "agent_id")]
    pub agent_id: String,
}

/// WriteManagedEnvProvenance persists the reuse-eligibility marker at the env
/// root. Callers must only invoke it for non-local_directory resumable envs,
/// since the file's presence is the non-local assertion. ManagedBy is stamped
/// here so callers cannot forget the discriminator.
pub fn write_managed_env_provenance(
    env_root: &str,
    mut p: ManagedEnvProvenance,
) -> anyhow::Result<()> {
    if env_root.is_empty() {
        return Ok(());
    }
    p.managed_by = MANAGED_ENV_PROVENANCE_MANAGED_BY.to_string();
    let data = serde_json::to_vec(&p).context("marshal managed env provenance")?;
    std::fs::write(Path::new(env_root).join(MANAGED_ENV_PROVENANCE_FILE), data)?;
    Ok(())
}

/// ReadManagedEnvProvenance reads the Prepare-time reuse-eligibility marker
/// from an env root. A missing or malformed file returns an error; callers
/// fail closed (no reuse) on any error.
pub fn read_managed_env_provenance(env_root: &str) -> anyhow::Result<ManagedEnvProvenance> {
    let data = std::fs::read(Path::new(env_root).join(MANAGED_ENV_PROVENANCE_FILE))?;
    Ok(serde_json::from_slice(&data)?)
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

impl Environment {
    /// Cleanup tears down the execution environment.
    /// If removeAll is true, the entire env root is deleted. Otherwise,
    /// workdir is removed but output/ and logs/ are preserved for debugging.
    ///
    /// For local_directory tasks (local_directory==true) work_dir is the
    /// user's own path — Cleanup MUST NEVER delete it, regardless of
    /// removeAll. In that mode we only ever delete the envRoot scratch
    /// directory.
    pub fn cleanup(&self, remove_all_flag: bool) -> anyhow::Result<()> {
        if self.local_directory {
            // Never touch the user's directory. RootDir is the daemon's own
            // scratch; safe to remove when the caller asked for a full
            // teardown.
            if remove_all_flag && !self.root_dir.is_empty() {
                if let Err(err) = remove_tree(&self.root_dir) {
                    tracing::warn!(
                        error = %format!("{err:#}"),
                        "execenv: cleanup local_directory envRoot failed"
                    );
                    return Err(err);
                }
            }
            return Ok(());
        }

        if remove_all_flag {
            if let Err(err) = remove_tree(&self.root_dir) {
                tracing::warn!(error = %format!("{err:#}"), "execenv: cleanup removeAll failed");
                return Err(err);
            }
            return Ok(());
        }

        // Partial cleanup: remove workdir, keep output/ and logs/.
        if let Err(err) = remove_tree(&self.work_dir) {
            tracing::warn!(error = %format!("{err:#}"), "execenv: cleanup workdir failed");
            return Err(err);
        }
        Ok(())
    }
}

/// remove_tree strips a file or directory tree, tolerating absence
/// (os.RemoveAll semantics) and surfacing other errors with context.
pub(crate) fn remove_tree(path: &str) -> anyhow::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            // Fall back to removing a plain file/symlink entry.
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(f) if f.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(f) => Err(anyhow::Error::new(f).context(format!("remove {path}"))),
            }
        }
    }
}

/// restrict_permissions applies chmod 0o700 on unix; on windows Go's
/// os.Chmod only toggles the read-only bit, which we deliberately skip.
fn restrict_permissions(path: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Env-root ownership
// ---------------------------------------------------------------------------

/// envRootOwnerFile records which task an env root belongs to. Creating it
/// with O_EXCL is the atomic step that decides which of two same-key tasks
/// owns the directory (#7326).
const ENV_ROOT_OWNER_FILE: &str = ".task_owner";

/// claimEnvRoot atomically establishes that taskID owns envRoot.
///
/// Returns true when this call created the claim and the directory is the
/// caller's to populate; false when taskID already owned it (a rerun that may
/// reset its own tree). Any other owner is an error, and so is a directory
/// that holds content but names no owner — refusing to guess is the whole
/// point of the marker.
///
/// Atomicity comes from two filesystem primitives rather than a lock: mkdir
/// fails if the directory exists, and O_EXCL fails if the marker exists.
/// Exactly one racing caller can win each, and a caller that wins neither
/// never reaches the destructive path.
pub(crate) fn claim_env_root(env_root: &str, task_id: &str) -> anyhow::Result<bool> {
    let root = Path::new(env_root);
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent).context("create workspace directory")?;
    }

    match std::fs::create_dir(root) {
        Ok(()) => {
            // We created the directory, so nothing of anyone's can be inside it.
            write_env_root_owner_exclusive(env_root, task_id)?;
            return Ok(true);
        }
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
            return Err(anyhow::Error::new(e).context(format!("create env root {env_root}")));
        }
        Err(_) => {}
    }

    // The directory already existed. Who owns it?
    let owner = read_env_root_owner(env_root)
        .map_err(|e| e.context(format!("read env root owner for {env_root}")))?;
    if owner == task_id {
        return Ok(false);
    }
    if !owner.is_empty() {
        bail!(
            "env root {env_root} belongs to task {owner}; refusing to reset it for task {task_id}"
        );
    }

    // No owner. A directory holding work is never ours to take — the marker
    // is written before any content, so the only unowned directory that can
    // be safely claimed is an empty one (a crash between Mkdir and the marker
    // write). Anything else fails closed and waits for a human.
    let empty = dir_is_empty(env_root).context(format!("inspect env root {env_root}"))?;
    if !empty {
        bail!(
            "env root {env_root} already holds files but names no owning task; refusing to delete it"
        );
    }
    // Lost the race for an empty directory: whoever won owns it now.
    write_env_root_owner_exclusive(env_root, task_id)?;
    Ok(true)
}

/// writeEnvRootOwnerExclusive creates the owner marker, failing if one
/// already exists. This is the atomic arbiter between two racing claims.
fn write_env_root_owner_exclusive(env_root: &str, task_id: &str) -> anyhow::Result<()> {
    let path = Path::new(env_root).join(ENV_ROOT_OWNER_FILE);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path);
    match f {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let owner = read_env_root_owner(env_root)
                .map_err(|e| e.context(format!("read env root owner for {env_root}")))?;
            if owner == task_id {
                return Ok(());
            }
            bail!(
                "env root {env_root} was claimed by task {owner} while task {task_id} was starting"
            );
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!("claim env root {env_root}")));
        }
        Ok(mut f) => {
            f.write_all(task_id.as_bytes()).map_err(|e| {
                anyhow::Error::new(e).context(format!("record env root owner for {env_root}"))
            })?;
        }
    }
    Ok(())
}

/// readEnvRootOwner returns the task id that owns envRoot, or "" when no
/// marker is present. An unreadable marker is an error, not an empty owner:
/// treating it as unowned would hand the caller a licence to delete the very
/// directory it could not identify.
fn read_env_root_owner(env_root: &str) -> anyhow::Result<String> {
    match std::fs::read_to_string(Path::new(env_root).join(ENV_ROOT_OWNER_FILE)) {
        Ok(content) => Ok(content.trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

/// resetEnvRootContents empties an env root the caller already owns, keeping
/// the directory and its owner marker. Removing and recreating the directory
/// instead would drop the claim for as long as the recreate takes, which is
/// exactly the window claimEnvRoot exists to close.
fn reset_env_root_contents(env_root: &str) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(env_root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy() == ENV_ROOT_OWNER_FILE {
            continue;
        }
        remove_tree(&entry.path().to_string_lossy())?;
    }
    Ok(())
}

/// dirIsEmpty reports whether dir has no entries at all.
fn dir_is_empty(dir: &str) -> anyhow::Result<bool> {
    Ok(std::fs::read_dir(dir)?.next().is_none())
}

// ---------------------------------------------------------------------------
// S9-integration: lane E2 provider seams
//
// These stand-ins keep prepare structurally identical to execenv.go while the
// hermes/openclaw/qwenpaw/reasonix provider families are ported in lane E2.
// Each fails closed so a mis-routed task surfaces loudly instead of running
// with missing configuration. E2 replaces these bodies (and deletes this
// section) without touching the call sites above.
// ---------------------------------------------------------------------------

/// OpenclawGatewayPin describes the Gateway endpoint a per-task openclaw
/// wrapper should pin (openclaw_config.go). Fields mirror OpenClaw's own
/// `gateway.*` config shape; only non-zero fields are emitted into the
/// wrapper. Zero means "inherit whatever the user's global openclaw.json
/// already configures".
///
/// Deviation vs Go: the public Go type masks Token in MarshalJSON and
/// Display. This stand-in serializes plainly (the isolation helper protocol
/// requires the real token over stdin anyway) and masks only in Display.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenclawGatewayPin {
    #[serde(rename = "host", skip_serializing_if = "String::is_empty")]
    pub host: String,
    #[serde(rename = "port", skip_serializing_if = "is_zero_i64")]
    pub port: i64,
    #[serde(rename = "token", skip_serializing_if = "String::is_empty")]
    pub token: String,
    #[serde(rename = "tls", skip_serializing_if = "std::ops::Not::not")]
    pub tls: bool,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

impl OpenclawGatewayPin {
    /// IsZero reports whether every field is zero, i.e. there is nothing to pin.
    pub fn is_zero(&self) -> bool {
        *self == OpenclawGatewayPin::default()
    }
}

impl std::fmt::Display for OpenclawGatewayPin {
    /// Masks the bearer token when the pin is rendered as a string (issue
    /// #3260 CR).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tok = if self.token.is_empty() { "" } else { "***" };
        write!(
            f,
            "OpenclawGatewayPin{{Host:{:?} Port:{} Token:{} TLS:{}}}",
            self.host, self.port, tok, self.tls
        )
    }
}

/// Result of preparing a Hermes overlay (hermes_home.go prepareHermesHome).
#[derive(Debug, Clone, Copy, Default)]
pub struct HermesSessions {
    pub mounted: bool,
    pub history_present: bool,
}

/// Config-prep inputs for openclaw (openclaw_config.go OpenclawConfigPrep).
#[derive(Debug, Clone, Default)]
pub struct OpenclawConfigPrep {
    pub openclaw_bin: String,
    pub mcp_config: Option<Value>,
    pub gateway: OpenclawGatewayPin,
}

/// Result of preparing the per-task OpenClaw config.
#[derive(Debug, Clone, Default)]
pub struct OpenclawConfigResult {
    pub config_path: String,
    pub include_root: String,
}

// S9-integration: hermes_home.go lands in lane E2.
#[allow(clippy::too_many_arguments)]
fn prepare_hermes_home(
    _hermes_home: &str,
    _source_home: &str,
    _source_must_exist: bool,
    _skills: &[SkillContextForEnv],
    _env: &HashMap<String, String>,
    _memory_store: &str,
    _session_store: &str,
) -> anyhow::Result<HermesSessions> {
    bail!("execenv: hermes provider family not yet ported (lane E2)")
}

/// Ensures the managed QwenPaw workspace root is a real directory.
///
/// `create_dir_all` follows a symlink when the final path already points to a
/// directory. That is unsafe for this managed path because all of the cleanup
/// and manifest writes below operate on descendants of it. Refuse symlinks and
/// other non-directory entries before touching any descendant.
fn ensure_qwenpaw_workspace_root(workspace: &str) -> anyhow::Result<()> {
    let path = Path::new(workspace);
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("execenv: qwenpaw workspace root must not be a symlink: {workspace}");
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("execenv: qwenpaw workspace root must be a directory: {workspace}");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)
                .with_context(|| format!("create qwenpaw workspace directory {workspace}"))?;
        }
        Err(error) => {
            return Err(anyhow::Error::new(error)
                .context(format!("inspect qwenpaw workspace root {workspace}")));
        }
    }

    // Re-check after creation so a path created between the initial probe and
    // create_dir_all cannot become an accepted symlink or non-directory.
    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("inspect qwenpaw workspace root {workspace}"))?;
    if metadata.file_type().is_symlink() {
        bail!("execenv: qwenpaw workspace root must not be a symlink: {workspace}");
    }
    if !metadata.is_dir() {
        bail!("execenv: qwenpaw workspace root must be a directory: {workspace}");
    }
    Ok(())
}

/// Prepares QwenPaw's per-task workspace and native skill manifest.
///
/// QwenPaw does not read Cordy's generic `.agent_context/skills` tree: its ACP
/// process discovers `<workspace>/skills` and enables entries listed by the
/// workspace `skill.json` manifest. Rebuilding both paths on every prepare (and
/// reuse) is therefore part of the provider contract, not an optional cache
/// refresh. Removing the old tree first also revokes a skill when an agent's
/// bindings change or become empty.
fn prepare_qwenpaw_workspace(workspace: &str, skills: &[SkillContextForEnv]) -> anyhow::Result<()> {
    if workspace.is_empty() {
        bail!("execenv: qwenpaw workspace is required");
    }

    ensure_qwenpaw_workspace_root(workspace)?;
    restrict_permissions(workspace)
        .with_context(|| format!("restrict qwenpaw workspace directory {workspace}"))?;

    let skills_dir = join_path(&[workspace, "skills"]);
    let manifest_path = join_path(&[workspace, "skill.json"]);
    remove_tree(&skills_dir)
        .with_context(|| format!("remove qwenpaw skills directory {skills_dir}"))?;
    remove_tree(&manifest_path)
        .with_context(|| format!("remove qwenpaw manifest {manifest_path}"))?;

    if skills.is_empty() {
        return Ok(());
    }

    // write_skill_files owns frontmatter normalization, collision-free slugs,
    // and supporting-file materialization shared by the other providers. The
    // QwenPaw tree was removed above, so its natural slug candidates are the
    // same ones represented in the manifest below.
    write_skill_files(&skills_dir, skills, None).context("write qwenpaw workspace skills")?;

    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut entries = serde_json::Map::new();
    for slug in resolve_skill_slugs(skills) {
        entries.insert(
            slug.clone(),
            serde_json::json!({
                "enabled": true,
                "channels": ["all"],
                "source": "customized",
                "metadata": {
                    "name": slug,
                    "description": "",
                    "source": "customized",
                    "protected": false,
                    "updated_at": now.clone(),
                },
                "updated_at": now.clone(),
            }),
        );
    }
    let manifest = serde_json::json!({
        "schema_version": "workspace-skill-manifest.v1",
        "version": 0,
        "skills": entries,
    });
    let data = serde_json::to_vec_pretty(&manifest).context("encode qwenpaw skill manifest")?;
    std::fs::write(&manifest_path, data)
        .with_context(|| format!("write qwenpaw skill manifest {manifest_path}"))?;
    tracing::info!(
        workspace,
        skills = skills.len(),
        "qwenpaw workspace prepared"
    );
    Ok(())
}

// Reasonix's full implementation lives in the capability module so the
// prepare/reuse call sites stay aligned with the other provider families.
fn write_reasonix_project_config(
    work_dir: &str,
    env: &HashMap<String, String>,
    manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<()> {
    reasonix::write_reasonix_project_config(work_dir, env, manifest)
}

// S9-integration: openclaw_config.go + openclaw_config_cache.go land in lane
// E2 (including openclawProfileCacheDir(profile)).
fn prepare_openclaw_config(
    _env_root: &str,
    _work_dir: &str,
    _prep: &OpenclawConfigPrep,
) -> anyhow::Result<OpenclawConfigResult> {
    bail!("execenv: openclaw provider family not yet ported (lane E2)")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestPredictRootDir (execenv_test.go).
    #[test]
    fn test_predict_root_dir() {
        assert_eq!(
            predict_root_dir("/tmp/ws", "ws1", "01a01ec0-e69d-7000-8000-0123456789ab"),
            "/tmp/ws/ws1/0123456789ab"
        );
        assert_eq!(predict_root_dir("", "ws1", "task"), "");
        assert_eq!(predict_root_dir("/tmp/ws", "", "task"), "");
        assert_eq!(predict_root_dir("/tmp/ws", "ws1", ""), "");
    }

    // join_path / clean_path must reproduce Go's filepath.Join/Clean cleaning
    // (collapsing separators, resolving `..`, "." elimination).
    #[test]
    fn test_join_path_cleaning() {
        assert_eq!(join_path(&["a", "b", "c"]), "a/b/c");
        assert_eq!(join_path(&["a//b", "c"]), "a/b/c");
        assert_eq!(join_path(&["a", "./b"]), "a/b");
        assert_eq!(join_path(&["a", "../b"]), "b");
        assert_eq!(join_path(&["/x/y", "..", "z"]), "/x/z");
        assert_eq!(join_path(&["", "", "a"]), "a");
        assert_eq!(join_path(&["", ""]), "");
        assert_eq!(join_path(&["a/", "b/"]), "a/b");
        assert_eq!(clean_path(""), ".");
        assert_eq!(clean_path("abc"), "abc");
        assert_eq!(clean_path("abc/def"), "abc/def");
        assert_eq!(clean_path("a/b/c/../.."), "a");
        assert_eq!(clean_path("../.."), "../..");
        assert_eq!(clean_path("/.."), "/");
        assert_eq!(clean_path("/../a"), "/a");
        assert_eq!(clean_path("./.."), "..");
    }

    // Port of TestGCMetaRoundTrip (execenv_test.go): marshal/unmarshal keeps
    // the discriminated union intact and pre-v2 files default to issue.
    #[test]
    fn test_gc_meta_round_trip_and_legacy_default() {
        let meta = GcMeta {
            kind: Some(GCMetaKind::Chat),
            chat_session_id: "chat_1".into(),
            workspace_id: "ws".into(),
            completed_at: Some(chrono::Utc::now()),
            ..Default::default()
        };
        let data = serde_json::to_vec(&meta).unwrap();
        let back: GcMeta = serde_json::from_slice(&data).unwrap();
        assert_eq!(back.kind, Some(GCMetaKind::Chat));
        assert_eq!(back.chat_session_id, "chat_1");

        // Pre-v2 file: no kind field at all.
        let legacy =
            br#"{"issue_id":"iss_1","workspace_id":"ws","completed_at":"2026-01-02T03:04:05Z"}"#;
        let back: GcMeta = serde_json::from_slice(legacy).unwrap();
        assert_eq!(back.kind, None);
        assert_eq!(back.issue_id, "iss_1");

        // Wire shape uses snake_case keys with omitempty.
        let v: Value = serde_json::from_slice(&data).unwrap();
        assert!(v.get("kind").is_some());
        assert!(v.get("issue_id").is_none(), "empty ids are omitted");
        assert!(v.get("workspace_id").is_some());
        assert!(v.get("completed_at").is_some());
    }

    // Port of TestGCMetaEmptyKindSkipsWrite.
    #[test]
    fn test_gc_meta_empty_kind_skips_write() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        write_gc_meta(&root, GcMeta::default()).unwrap();
        assert!(!Path::new(&root).join(GC_META_FILE).exists());

        let meta = GcMeta {
            kind: Some(GCMetaKind::Issue),
            issue_id: "iss".into(),
            workspace_id: "ws".into(),
            ..Default::default()
        };
        write_gc_meta(&root, meta.clone()).unwrap();
        let read = read_gc_meta(&root).unwrap();
        assert_eq!(read.kind, Some(GCMetaKind::Issue));
        assert_eq!(read.issue_id, "iss");
        assert!(read.completed_at.is_some(), "CompletedAt stamped by writer");
    }

    // Port of TestManagedEnvProvenanceRoundTrip.
    #[test]
    fn test_managed_env_provenance_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        write_managed_env_provenance(
            &root,
            ManagedEnvProvenance {
                workspace_id: "ws".into(),
                issue_id: "iss".into(),
                agent_id: "ag".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let back = read_managed_env_provenance(&root).unwrap();
        assert_eq!(back.managed_by, MANAGED_ENV_PROVENANCE_MANAGED_BY);
        assert_eq!(back.workspace_id, "ws");
        assert_eq!(back.issue_id, "iss");
        assert_eq!(back.chat_session_id, "");

        // Empty env root is a no-op, matching Go.
        write_managed_env_provenance("", ManagedEnvProvenance::default()).unwrap();
    }

    // Port of TestClaimEnvRootOwnership (execenv_test.go core cases).
    #[test]
    fn test_claim_env_root_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_string_lossy().to_string();
        let root = join_path(&[&base, "env"]);

        // Fresh claim wins.
        assert!(claim_env_root(&root, "t1").unwrap());
        assert_eq!(read_env_root_owner_pub(&root), "t1");

        // Same task re-claims: not fresh, allowed to reset.
        assert!(!claim_env_root(&root, "t1").unwrap());

        // Another task is refused outright.
        let err = claim_env_root(&root, "t2").unwrap_err();
        assert!(
            format!("{err:#}").contains("belongs to task t1"),
            "unexpected error: {err:#}"
        );

        // Unowned but non-empty directory refuses to be claimed.
        let orphan = join_path(&[&base, "orphan"]);
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(join_path(&[&orphan, "precious.txt"]), b"data").unwrap();
        let err = claim_env_root(&orphan, "t1").unwrap_err();
        assert!(
            format!("{err:#}").contains("already holds files but names no owning task"),
            "unexpected error: {err:#}"
        );

        // Unowned EMPTY directory can be claimed (crash-between-mkdir-and-
        // marker recovery).
        let empty = join_path(&[&base, "empty"]);
        std::fs::create_dir_all(&empty).unwrap();
        assert!(claim_env_root(&empty, "t1").unwrap());
    }

    fn read_env_root_owner_pub(root: &str) -> String {
        std::fs::read_to_string(Path::new(root).join(ENV_ROOT_OWNER_FILE))
            .unwrap()
            .trim()
            .to_string()
    }

    // Port of TestResetEnvRootContentsKeepsOwnerMarker.
    #[test]
    fn test_reset_env_root_contents_keeps_owner_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_string_lossy().to_string();
        let root = join_path(&[&base, "env"]);
        assert!(claim_env_root(&root, "t1").unwrap());
        std::fs::create_dir_all(join_path(&[&root, "workdir"])).unwrap();
        std::fs::write(join_path(&[&root, "workdir", "f.txt"]), b"x").unwrap();

        reset_env_root_contents(&root).unwrap();
        assert_eq!(read_env_root_owner_pub(&root), "t1");
        assert!(!Path::new(&root).join("workdir").exists());
    }

    // Port of TestProjectResourceMarshalShape: resource_ref stays raw and
    // defaults to {}; label carries omitempty.
    #[test]
    fn test_project_resource_marshal_shape() {
        let r = ProjectResourceForEnv {
            id: "r1".into(),
            resource_type: "github_repo".into(),
            resource_ref: Some(serde_json::json!({"url": "https://example.com/repo"})),
            label: String::new(),
        };
        let v: Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["resource_type"], "github_repo");
        assert_eq!(v["resource_ref"]["url"], "https://example.com/repo");
        assert!(v.get("label").is_none(), "empty label omitted");

        let no_ref = ProjectResourceForEnv {
            id: "r2".into(),
            resource_type: "local_directory".into(),
            resource_ref: None,
            label: "mine".into(),
        };
        let v: Value = serde_json::to_value(&no_ref).unwrap();
        assert_eq!(v["resource_ref"], serde_json::json!({}));
        assert_eq!(v["label"], "mine");
    }

    #[test]
    fn qwenpaw_workspace_writes_native_skills_and_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("qwenpaw");
        let skills = vec![
            SkillContextForEnv {
                name: "Review Helper".into(),
                description: "Reviews changes".into(),
                content: "# Review Helper\n\nReview body".into(),
                files: vec![SkillFileContextForEnv {
                    path: "scripts/check.py".into(),
                    content: "print('ok')".into(),
                }],
            },
            SkillContextForEnv {
                name: "Bug Finder".into(),
                content: "# Bug Finder\n\nFind bugs".into(),
                ..Default::default()
            },
        ];

        prepare_qwenpaw_workspace(workspace.to_str().unwrap(), &skills).unwrap();

        assert!(workspace.join("skills/review-helper/SKILL.md").is_file());
        assert_eq!(
            std::fs::read_to_string(workspace.join("skills/review-helper/scripts/check.py"))
                .unwrap(),
            "print('ok')"
        );
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(workspace.join("skill.json")).unwrap()).unwrap();
        assert_eq!(manifest["schema_version"], "workspace-skill-manifest.v1");
        assert_eq!(manifest["version"], 0);
        assert_eq!(manifest["skills"]["review-helper"]["enabled"], true);
        assert_eq!(manifest["skills"]["review-helper"]["channels"][0], "all");
        assert_eq!(
            manifest["skills"]["review-helper"]["metadata"]["name"],
            "review-helper"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&workspace).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn qwenpaw_workspace_rebuild_revokes_removed_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("qwenpaw");
        let skill = SkillContextForEnv {
            name: "Deploy Helper".into(),
            content: "# Deploy Helper".into(),
            ..Default::default()
        };

        prepare_qwenpaw_workspace(workspace.to_str().unwrap(), &[skill]).unwrap();
        assert!(workspace.join("skills/deploy-helper").exists());
        assert!(workspace.join("skill.json").exists());

        prepare_qwenpaw_workspace(workspace.to_str().unwrap(), &[]).unwrap();
        assert!(!workspace.join("skills").exists());
        assert!(!workspace.join("skill.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn qwenpaw_workspace_rejects_symlinked_root() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir_all(target.join("skills")).unwrap();
        std::fs::write(target.join("skills/keep.txt"), b"keep").unwrap();
        let workspace = tmp.path().join("qwenpaw");
        symlink(&target, &workspace).unwrap();

        let err = prepare_qwenpaw_workspace(workspace.to_str().unwrap(), &[]).unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("must not be a symlink"),
            "unexpected error: {message}"
        );
        assert!(target.join("skills/keep.txt").is_file());
        assert!(workspace
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn qwenpaw_workspace_rejects_non_directory_root() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("qwenpaw");
        std::fs::write(&workspace, b"not a directory").unwrap();

        let err = prepare_qwenpaw_workspace(workspace.to_str().unwrap(), &[]).unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("must be a directory"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn qwenpaw_workspace_deduplicates_skill_slugs() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("qwenpaw");
        let skills = vec![
            SkillContextForEnv {
                name: "A B".into(),
                content: "# First".into(),
                ..Default::default()
            },
            SkillContextForEnv {
                name: "A-B".into(),
                content: "# Second".into(),
                ..Default::default()
            },
        ];

        prepare_qwenpaw_workspace(workspace.to_str().unwrap(), &skills).unwrap();
        assert!(workspace.join("skills/a-b/SKILL.md").is_file());
        assert!(workspace.join("skills/a-b-cordy/SKILL.md").is_file());
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(workspace.join("skill.json")).unwrap()).unwrap();
        assert_eq!(manifest["skills"].as_object().unwrap().len(), 2);
        assert_eq!(
            manifest["skills"]["a-b-cordy"]["metadata"]["name"],
            "a-b-cordy"
        );
    }

    // Port of TestOpenclawGatewayPinZeroAndMasking.
    #[test]
    fn test_openclaw_gateway_pin_zero_and_masking() {
        assert!(OpenclawGatewayPin::default().is_zero());
        let pin = OpenclawGatewayPin {
            host: "gw.internal".into(),
            port: 7420,
            token: "sekrit".into(),
            tls: true,
        };
        assert!(!pin.is_zero());
        let rendered = format!("{pin}");
        assert!(rendered.contains("***"), "token masked: {rendered}");
        assert!(!rendered.contains("sekrit"), "token leaked: {rendered}");
        // Wire form keeps the token (helper protocol needs it).
        let v: Value = serde_json::to_value(&pin).unwrap();
        assert_eq!(v["token"], "sekrit");
    }
}
