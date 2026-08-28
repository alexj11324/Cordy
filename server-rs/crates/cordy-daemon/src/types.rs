//! Daemon-side wire types for the claim/report protocol.
//!
//! Symbol map (Go → Rust):
//! - `AgentEntry` → [`AgentEntry`]
//! - `Runtime` → [`Runtime`]
//! - `RepoData` → [`RepoData`]
//! - `ProjectResourceData` → [`ProjectResourceData`]
//! - `ConnectedAppData = runtimeapps.ConnectedApp` → alias to the execenv
//!   stand-in ([`ConnectedAppData`])
//! - `ActiveSiblingRunData` → [`ActiveSiblingRunData`]
//! - `Task` → [`Task`]
//! - `ChatAttachmentMeta` → [`ChatAttachmentMeta`]
//! - `CoalescedCommentData` → [`CoalescedCommentData`]
//! - `AgentData` → [`AgentData`]
//! - `DisabledRuntimeSkillData` → [`DisabledRuntimeSkillData`]
//! - `SkillData` / `SkillFileData` / `SkillRefData` / `SkillFileRefData` →
//!   same-named structs
//! - `TaskUsageEntry` → [`TaskUsageEntry`]
//! - `TaskResult` → [`TaskResult`]
//! - `PluginHookTool` → [`PluginHookTool`]
//!
//! Remote MCP claim fields reuse the shared `cordy-remotemcp` wire types;
//! `ConnectedAppData` aliases the execenv ConnectedApp shape.

use serde::{Deserialize, Serialize};

use crate::execenv::execenv::ConnectedApp;

pub type RemoteMcpTool = cordy_remotemcp::Tool;
pub type RemoteMcpConnection = cordy_remotemcp::Connection;

/// AgentEntry describes a single available agent CLI (types.go:11–22).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentEntry {
    /// Stable startup-resolved CLI entry point; launch resolution may follow
    /// platform links to a concrete path.
    pub path: String,
    /// The bare command name or CORDY_*_PATH value that Path was resolved from
    /// at startup. Kept so the daemon can re-resolve Path if the pinned
    /// executable later vanishes — e.g. a version manager (Homebrew Cask,
    /// nvm/fnm) does an in-place upgrade that deletes the old versioned
    /// directory Path points into. Empty for synthesized entries (custom
    /// runtime profiles) that carry an absolute path directly. See
    /// Daemon.resolveAgentEntry and PB-4486.
    pub command: String,
    /// Model override (optional).
    pub model: String,
}

/// Runtime represents a registered daemon runtime (types.go:25–36).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "status")]
    pub status: String,
    /// ProfileID is non-empty when this runtime was registered from a
    /// workspace custom runtime profile (PB-3284). It links the runtime row
    /// back to the profile so the daemon can resolve the profile's command_name
    /// to the executable to launch. Built-in (provider-detected) runtimes
    /// leave this empty.
    #[serde(
        rename = "profile_id",
        default,
        deserialize_with = "deserialize_null_string",
        skip_serializing_if = "String::is_empty"
    )]
    pub profile_id: String,
}

/// Immutable execution identity selected from the accepted registration row.
/// Provider family and custom profile travel together: the latter owns the
/// machine-specific executable and fixed launch prefix for custom runtimes,
/// and cannot be reconstructed from a claimed task alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeExecutionTarget {
    pub provider: String,
    pub profile_id: String,
}

/// RepoData holds repository information from the workspace (types.go:39–43).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepoData {
    #[serde(rename = "url")]
    pub url: String,
    #[serde(rename = "description", skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "ref", skip_serializing_if = "String::is_empty")]
    pub ref_: String,
}

/// ProjectResourceData mirrors handler.ProjectResourceData — a single project
/// resource as delivered to the daemon. resource_ref is type-specific JSON
/// (types.go:46–52).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectResourceData {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "resource_type")]
    pub resource_type: String,
    #[serde(rename = "resource_ref")]
    pub resource_ref: serde_json::Value,
    #[serde(rename = "label", skip_serializing_if = "String::is_empty")]
    pub label: String,
}

/// ConnectedAppData keeps the claim-response field local to daemon types while
/// sharing the canonical JSON shape with the runtime app metadata package
/// (types.go:56).
pub type ConnectedAppData = ConnectedApp;

/// ActiveSiblingRunData mirrors the claim-time warning context returned by the
/// server for another in-flight issue task owned by this agent. Queued tasks
/// are intentionally excluded from this context (types.go:59–69).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActiveSiblingRunData {
    #[serde(rename = "task_id")]
    pub task_id: String,
    #[serde(rename = "issue_id")]
    pub issue_id: String,
    #[serde(rename = "issue_identifier")]
    pub issue_identifier: String,
    #[serde(rename = "issue_title")]
    pub issue_title: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "created_at")]
    pub created_at: String,
    #[serde(rename = "started_at", skip_serializing_if = "String::is_empty")]
    pub started_at: String,
}

/// Task represents a claimed task from the server (types.go:72–169). Agent data
/// (name, skills) is populated by the claim endpoint.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "agent_id")]
    pub agent_id: String,
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(rename = "issue_id")]
    pub issue_id: String,
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(
        rename = "remote_mcp_connections",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub remote_mcp_connections: Vec<RemoteMcpConnection>,
    /// RemoteMCPDaemonToken stays inside the daemon and authenticates the
    /// local broker's credential-resolution calls. It must never enter agent
    /// env/config.
    #[serde(
        rename = "remote_mcp_daemon_token",
        skip_serializing_if = "String::is_empty"
    )]
    pub remote_mcp_daemon_token: String,
    /// PluginHookTools are this workspace's agent-trigger plugin hooks, which
    /// the local MCP server presents to the agent as tools. Resolved by the
    /// server at claim time; the daemon never reads plugin state itself.
    #[serde(rename = "plugin_hook_tools", skip_serializing_if = "Vec::is_empty")]
    pub plugin_hook_tools: Vec<PluginHookTool>,
    /// WorkspaceContext mirrors workspace.context (the per-workspace system
    /// prompt set in Settings → General). Server populates this on every claim
    /// regardless of task kind so the daemon can inject `## Workspace Context`
    /// into the brief. Empty when the owner hasn't set one.
    #[serde(rename = "workspace_context", skip_serializing_if = "String::is_empty")]
    pub workspace_context: String,
    #[serde(rename = "active_sibling_runs", skip_serializing_if = "Vec::is_empty")]
    pub active_sibling_runs: Vec<ActiveSiblingRunData>,
    /// Semantic title for provider-native session/thread history.
    #[serde(rename = "thread_name", skip_serializing_if = "String::is_empty")]
    pub thread_name: String,
    #[serde(rename = "agent", skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentData>,
    /// Per-run app capabilities mounted through runtime MCP overlays.
    #[serde(rename = "connected_apps", skip_serializing_if = "Vec::is_empty")]
    pub connected_apps: Vec<ConnectedAppData>,
    #[serde(rename = "repos", skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoData>,
    /// Active project for this task, when present.
    #[serde(rename = "project_id", skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    /// Human-readable project title for context injection.
    #[serde(rename = "project_title", skip_serializing_if = "String::is_empty")]
    pub project_title: String,
    /// Durable project-level context injected into the brief.
    #[serde(
        rename = "project_description",
        skip_serializing_if = "String::is_empty"
    )]
    pub project_description: String,
    /// Project-scoped resources to expose to the agent.
    #[serde(rename = "project_resources", skip_serializing_if = "Vec::is_empty")]
    pub project_resources: Vec<ProjectResourceData>,
    /// True when executing in the squad-leader coordinator role.
    #[serde(rename = "is_leader_task", skip_serializing_if = "std::ops::Not::not")]
    pub is_leader_task: bool,
    /// Server capability: IsLeaderTask/SquadID authoritatively answer "is this
    /// a leader run". Absent on servers predating it — those before #4951
    /// never sent is_leader_task at all, later ones send it without this
    /// guarantee — so taskIsSquadLeader falls back to the briefing marker for
    /// both (PB-5811).
    #[serde(
        rename = "leader_role_resolved",
        skip_serializing_if = "std::ops::Not::not",
        default
    )]
    pub leader_role_resolved: bool,
    /// Claude session ID from a previous task on this issue.
    #[serde(rename = "prior_session_id", skip_serializing_if = "String::is_empty")]
    pub prior_session_id: String,
    /// work_dir from a previous task on this issue.
    #[serde(rename = "prior_work_dir", skip_serializing_if = "String::is_empty")]
    pub prior_work_dir: String,
    /// PB-5305: server signals a more recent Codex session was withheld
    /// (rollout missing) and PriorSessionID (if any) is an older fallback; the
    /// run must disclose the continuity gap even if that older session resumes
    /// cleanly. Absent/false on old servers.
    #[serde(
        rename = "prior_session_resume_unavailable",
        skip_serializing_if = "std::ops::Not::not",
        default
    )]
    pub prior_session_resume_unavailable: bool,
    /// Comment that triggered this task.
    #[serde(
        rename = "trigger_comment_id",
        skip_serializing_if = "String::is_empty"
    )]
    pub trigger_comment_id: String,
    /// PB-4195: earlier comments folded into this run while it was still
    /// queued; the agent must address these in addition to the (newest)
    /// triggering comment. Empty for old servers / non-merged runs.
    #[serde(
        rename = "coalesced_comment_ids",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub coalesced_comment_ids: Vec<String>,
    /// PB-4195: full detail of the folded comments
    /// (thread_id/author/created_at/content) so the prompt can address each
    /// without assuming a shared thread. Empty for old servers / non-merged
    /// runs.
    #[serde(rename = "coalesced_comments", skip_serializing_if = "Vec::is_empty")]
    pub coalesced_comments: Vec<CoalescedCommentData>,
    /// Root comment ID for the triggering thread; falls back to
    /// trigger_comment_id on old servers.
    #[serde(rename = "trigger_thread_id", skip_serializing_if = "String::is_empty")]
    pub trigger_thread_id: String,
    /// Content of the triggering comment.
    #[serde(
        rename = "trigger_comment_content",
        skip_serializing_if = "String::is_empty"
    )]
    pub trigger_comment_content: String,
    /// "agent" or "member" — author kind for the triggering comment.
    #[serde(
        rename = "trigger_author_type",
        skip_serializing_if = "String::is_empty"
    )]
    pub trigger_author_type: String,
    /// Display name of the triggering comment author.
    #[serde(
        rename = "trigger_author_name",
        skip_serializing_if = "String::is_empty"
    )]
    pub trigger_author_name: String,
    /// Issue-wide comments since this agent's last run (excludes its own and
    /// the injected trigger); 0/omitted for old daemons or cold start.
    #[serde(rename = "new_comment_count", skip_serializing_if = "is_zero_i64")]
    pub new_comment_count: i64,
    /// RFC3339 anchor (last run's started_at) the count is measured from;
    /// empty on cold start.
    #[serde(
        rename = "new_comments_since",
        skip_serializing_if = "String::is_empty"
    )]
    pub new_comments_since: String,
    /// Non-empty for chat tasks.
    #[serde(rename = "chat_session_id", skip_serializing_if = "String::is_empty")]
    pub chat_session_id: String,
    /// "slack" when the chat session is backed by an IM channel; empty for a
    /// web-only chat. Drives the channel-awareness block in the prompt.
    #[serde(rename = "chat_channel_type", skip_serializing_if = "String::is_empty")]
    pub chat_channel_type: String,
    /// Server capability: this deployment carries a file the agent produces
    /// the last hop into this conversation. Absent on a server predating it,
    /// which reads as false — the run is told to describe its file in words,
    /// and the worst case is a delivery that could have happened did not.
    /// Must never be re-derived from chat_channel_type: whether the hop exists
    /// depends on the SERVER's storage and adapter wiring, which no daemon can
    /// see (PB-4899).
    #[serde(
        rename = "chat_channel_delivers_files",
        skip_serializing_if = "std::ops::Not::not",
        default
    )]
    pub chat_channel_delivers_files: bool,
    /// "group" when the channel conversation is a shared room, "p2p" for a
    /// 1:1 with the bot. Empty for a web chat or an old server; the per-turn
    /// prompt then reports unknown rather than guessing 1:1.
    #[serde(rename = "chat_type", skip_serializing_if = "String::is_empty")]
    pub chat_type: String,
    /// True when the latest @mention was a thread reply; selects which read
    /// command the prompt tells the agent to start with.
    #[serde(rename = "chat_in_thread", skip_serializing_if = "std::ops::Not::not")]
    pub chat_in_thread: bool,
    /// User message content for chat tasks.
    #[serde(rename = "chat_message", skip_serializing_if = "String::is_empty")]
    pub chat_message: String,
    /// Attachments linked to the chat message; agent uses these to
    /// `cordy attachment download <id>`.
    #[serde(
        rename = "chat_message_attachments",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub chat_message_attachments: Vec<ChatAttachmentMeta>,
    /// Legacy compatibility for historical is_agent_intro sessions; new agent
    /// creation no longer creates these chats.
    #[serde(rename = "chat_intro", skip_serializing_if = "std::ops::Not::not")]
    pub chat_intro: bool,
    /// Set only by servers predating server-side quick-actions generation
    /// (PB-5573). Read as a REFUSAL marker, never executed: see the guard in
    /// runTask.
    #[serde(
        rename = "regenerate_quick_actions_for",
        skip_serializing_if = "String::is_empty"
    )]
    pub regenerate_quick_actions_for: String,
    /// Non-empty for autopilot run_only tasks.
    #[serde(rename = "autopilot_run_id", skip_serializing_if = "String::is_empty")]
    pub autopilot_run_id: String,
    /// Autopilot that spawned this run.
    #[serde(rename = "autopilot_id", skip_serializing_if = "String::is_empty")]
    pub autopilot_id: String,
    /// Autopilot title used as task context.
    #[serde(rename = "autopilot_title", skip_serializing_if = "String::is_empty")]
    pub autopilot_title: String,
    /// Autopilot description used as task prompt.
    #[serde(
        rename = "autopilot_description",
        skip_serializing_if = "String::is_empty"
    )]
    pub autopilot_description: String,
    /// Manual, schedule, webhook, or api.
    #[serde(rename = "autopilot_source", skip_serializing_if = "String::is_empty")]
    pub autopilot_source: String,
    /// Optional trigger payload for webhook/api runs.
    #[serde(
        rename = "autopilot_trigger_payload",
        skip_serializing_if = "Option::is_none"
    )]
    pub autopilot_trigger_payload: Option<serde_json::Value>,
    /// User's natural-language input for quick-create tasks.
    #[serde(
        rename = "quick_create_prompt",
        skip_serializing_if = "String::is_empty"
    )]
    pub quick_create_prompt: String,
    /// Explicit priority selected in quick-create.
    #[serde(
        rename = "quick_create_priority",
        skip_serializing_if = "String::is_empty"
    )]
    pub quick_create_priority: String,
    /// Explicit calendar due date selected in quick-create.
    #[serde(
        rename = "quick_create_due_date",
        skip_serializing_if = "String::is_empty"
    )]
    pub quick_create_due_date: String,
    /// Attachments uploaded in the quick-create prompt and bound by issue
    /// create.
    #[serde(
        rename = "quick_create_attachment_ids",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub quick_create_attachment_ids: Vec<String>,
    /// Assignment handoff instruction; rendered into the opening prompt +
    /// issue_context.md.
    #[serde(rename = "handoff_note", skip_serializing_if = "String::is_empty")]
    pub handoff_note: String,

    /// When the picker was a squad, the squad's UUID; Agent is still the
    /// resolved leader.
    #[serde(rename = "squad_id", skip_serializing_if = "String::is_empty")]
    pub squad_id: String,
    /// Display name for the picker squad, used in prompt text.
    #[serde(rename = "squad_name", skip_serializing_if = "String::is_empty")]
    pub squad_name: String,
    /// For quick-create tasks opened from "Add sub issue" — UUID of the parent
    /// issue the new issue should be filed under.
    #[serde(rename = "parent_issue_id", skip_serializing_if = "String::is_empty")]
    pub parent_issue_id: String,
    /// Human-readable identifier (e.g. PB-123) of the quick-create parent
    /// issue, used in prompt context.
    #[serde(
        rename = "parent_issue_identifier",
        skip_serializing_if = "String::is_empty"
    )]
    pub parent_issue_identifier: String,
    /// RequestingUserName + RequestingUserProfileDescription describe the human
    /// the agent is working on behalf of. v1 sources them from the runtime
    /// owner (the user who registered the daemon). Empty when the runtime has
    /// no owner (cloud / system runtimes) or the user hasn't set a description.
    /// Injected into the brief under `## Requesting User`; omitted entirely
    /// when description is empty so the agent doesn't see a useless heading.
    #[serde(
        rename = "requesting_user_name",
        skip_serializing_if = "String::is_empty"
    )]
    pub requesting_user_name: String,
    #[serde(
        rename = "requesting_user_profile_description",
        skip_serializing_if = "String::is_empty"
    )]
    pub requesting_user_profile_description: String,
    /// Initiator* identify the actor who triggered THIS task (the real
    /// requester behind the current comment/mention or chat message) as
    /// distinct from the runtime owner whose credentials the agent runs with.
    /// Comment-triggered tasks resolve to the triggering comment's author;
    /// chat tasks resolve to the chat session creator. Empty for task kinds
    /// with no attributable human initiator (on-assign, autopilot,
    /// quick-create). InitiatorEmail is set only for member initiators. The
    /// daemon emits these into the brief under `## Task Initiator` so a
    /// workspace-visible agent can attribute the request per person. The
    /// agent's effective credentials stay owner-scoped — this is an attested
    /// identity, not a credential. See PB-2645.
    #[serde(rename = "initiator_type", skip_serializing_if = "String::is_empty")]
    pub initiator_type: String,
    #[serde(rename = "initiator_id", skip_serializing_if = "String::is_empty")]
    pub initiator_id: String,
    #[serde(rename = "initiator_name", skip_serializing_if = "String::is_empty")]
    pub initiator_name: String,
    #[serde(rename = "initiator_email", skip_serializing_if = "String::is_empty")]
    pub initiator_email: String,
    /// AuthToken is the task-scoped credential the server mints at claim time.
    /// The daemon injects it into the spawned agent as CORDY_TOKEN so the
    /// agent never sees the daemon's own (often workspace-owner) credential.
    /// Empty or non-task-scoped values are fatal for writable agent tasks; the
    /// daemon must not fall back to its own token. See PB-3292.
    #[serde(rename = "auth_token", skip_serializing_if = "String::is_empty")]
    pub auth_token: String,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Task")
            .field("id", &self.id)
            .field("agent_id", &self.agent_id)
            .field("runtime_id", &self.runtime_id)
            .field("workspace_id", &self.workspace_id)
            .field("issue_id", &self.issue_id)
            .field("chat_session_id", &self.chat_session_id)
            .field("autopilot_run_id", &self.autopilot_run_id)
            .field(
                "has_quick_create_prompt",
                &!self.quick_create_prompt.is_empty(),
            )
            .field("agent", &self.agent)
            .field("repo_count", &self.repos.len())
            .field("project_resource_count", &self.project_resources.len())
            .field("has_task_auth_token", &!self.auth_token.is_empty())
            .field(
                "has_remote_mcp_daemon_token",
                &!self.remote_mcp_daemon_token.is_empty(),
            )
            .finish_non_exhaustive()
    }
}

/// Serde helper mirroring Go's `omitempty` for int fields (0 omitted).
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

fn deserialize_null_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_accepts_omitted_compatibility_fields() {
        let task: Task = serde_json::from_str(r#"{"id":"task-1"}"#).unwrap();

        assert_eq!(task.id, "task-1");
        assert!(task.thread_name.is_empty());
        assert!(task.remote_mcp_connections.is_empty());
        assert!(task.agent.is_none());
    }

    #[test]
    fn runtime_accepts_null_profile_id() {
        let runtime: Runtime = serde_json::from_str(
            r#"{"id":"runtime-1","name":"local","provider":"codex","status":"online","profile_id":null}"#,
        )
        .unwrap();

        assert!(runtime.profile_id.is_empty());
    }

    #[test]
    fn task_and_agent_debug_redact_claim_credentials() {
        let task = Task {
            id: "task-1".to_string(),
            auth_token: "mat_task_secret".to_string(),
            remote_mcp_daemon_token: "daemon_secret".to_string(),
            agent: Some(AgentData {
                custom_env: Some(std::collections::HashMap::from([(
                    "API_KEY".to_string(),
                    "env_secret".to_string(),
                )])),
                custom_args: vec!["--token=arg_secret".to_string()],
                mcp_config: Some(serde_json::json!({"token":"mcp_secret"})),
                runtime_config: Some(serde_json::json!({"token":"runtime_secret"})),
                ..AgentData::default()
            }),
            ..Task::default()
        };

        let rendered = format!("{task:?}");
        for secret in [
            "mat_task_secret",
            "daemon_secret",
            "env_secret",
            "arg_secret",
            "mcp_secret",
            "runtime_secret",
        ] {
            assert!(!rendered.contains(secret), "Debug leaked {secret}");
        }
    }
}

/// ChatAttachmentMeta is the structured attachment metadata the daemon hands
/// to the agent for chat tasks (types.go:172–180). We pass id + filename +
/// content_type so the chat prompt can list them explicitly and instruct the
/// agent to run `cordy attachment download <id>` instead of guessing from a
/// signed CDN URL (which expires).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatAttachmentMeta {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "filename")]
    pub filename: String,
    #[serde(rename = "content_type", skip_serializing_if = "String::is_empty")]
    pub content_type: String,
}

/// CoalescedCommentData mirrors the server-side struct
/// (handler.CoalescedCommentData): the full detail of a comment folded into
/// this run while it was still queued (PB-4195) (types.go:183–193). The
/// prompt embeds each one directly so the agent addresses every folded comment
/// without assuming they all live in the triggering thread.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoalescedCommentData {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "thread_id", skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(rename = "author_type", skip_serializing_if = "String::is_empty")]
    pub author_type: String,
    #[serde(rename = "author_name", skip_serializing_if = "String::is_empty")]
    pub author_name: String,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "created_at", skip_serializing_if = "String::is_empty")]
    pub created_at: String,
}

/// AgentData holds agent details returned by the claim endpoint
/// (types.go:196–214).
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentData {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "instructions")]
    pub instructions: String,
    #[serde(rename = "skills", skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillData>,
    #[serde(rename = "skill_refs", skip_serializing_if = "Vec::is_empty")]
    pub skill_refs: Vec<SkillRefData>,
    #[serde(rename = "custom_env", skip_serializing_if = "Option::is_none")]
    pub custom_env: Option<std::collections::HashMap<String, String>>,
    #[serde(rename = "custom_args", skip_serializing_if = "Vec::is_empty")]
    pub custom_args: Vec<String>,
    #[serde(rename = "mcp_config", skip_serializing_if = "Option::is_none")]
    pub mcp_config: Option<serde_json::Value>,
    #[serde(rename = "model", skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(rename = "thinking_level", skip_serializing_if = "String::is_empty")]
    pub thinking_level: String,
    #[serde(rename = "service_tier", skip_serializing_if = "String::is_empty")]
    pub service_tier: String,
    #[serde(
        rename = "disabled_runtime_skills",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub disabled_runtime_skills: Vec<DisabledRuntimeSkillData>,
    /// RuntimeConfig is the per-provider runtime_config JSON as stored on the
    /// agent record, forwarded verbatim by the claim endpoint. The daemon
    /// decodes provider-specific fields (e.g. openclaw mode + gateway
    /// endpoint, see issue #3260); other backends ignore it.
    #[serde(rename = "runtime_config", skip_serializing_if = "Option::is_none")]
    pub runtime_config: Option<serde_json::Value>,
}

impl std::fmt::Debug for AgentData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentData")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("skill_count", &self.skills.len())
            .field("skill_ref_count", &self.skill_refs.len())
            .field(
                "custom_env_variable_count",
                &self
                    .custom_env
                    .as_ref()
                    .map_or(0, std::collections::HashMap::len),
            )
            .field("custom_arg_count", &self.custom_args.len())
            .field("has_mcp_config", &self.mcp_config.is_some())
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("service_tier", &self.service_tier)
            .field("has_runtime_config", &self.runtime_config.is_some())
            .finish_non_exhaustive()
    }
}

/// DisabledRuntimeSkillData is the task-wire identity of one runtime-local
/// skill that must be hidden from this agent's provider process
/// (types.go:218–225).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DisabledRuntimeSkillData {
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "root")]
    pub root: String,
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "name", skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(rename = "plugin", skip_serializing_if = "String::is_empty")]
    pub plugin: String,
}

/// SkillData represents a structured skill for task execution
/// (types.go:228–237).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillData {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "source", skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "description", skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "hash", skip_serializing_if = "String::is_empty")]
    pub hash: String,
    #[serde(rename = "size_bytes", skip_serializing_if = "is_zero_i64")]
    pub size_bytes: i64,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "files", skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SkillFileData>,
}

/// SkillFileData represents a supporting file within a skill
/// (types.go:240–245).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillFileData {
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "sha256", skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(rename = "size_bytes", skip_serializing_if = "is_zero_i64")]
    pub size_bytes: i64,
}

/// SkillRefData (types.go:248–256).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillRefData {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "source")]
    pub source: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "description", skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "hash")]
    pub hash: String,
    #[serde(rename = "size_bytes")]
    pub size_bytes: i64,
    #[serde(rename = "file_count")]
    pub file_count: i64,
    #[serde(rename = "files", skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SkillFileRefData>,
}

/// SkillFileRefData (types.go:259–262).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillFileRefData {
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "sha256")]
    pub sha256: String,
    #[serde(rename = "size_bytes")]
    pub size_bytes: i64,
}

/// TaskUsageEntry represents token usage for a single model during a task
/// execution (types.go:265–277).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskUsageEntry {
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "model")]
    pub model: String,
    #[serde(rename = "input_tokens")]
    pub input_tokens: i64,
    #[serde(rename = "output_tokens")]
    pub output_tokens: i64,
    #[serde(rename = "cache_read_tokens")]
    pub cache_read_tokens: i64,
    #[serde(rename = "cache_write_tokens")]
    pub cache_write_tokens: i64,
    /// CostUSDTicks is the provider's own price for this usage, in 1e-10 USD.
    /// Omitted when the agent reports no cost, which is the common case — the
    /// server then leaves the column NULL and the client estimates from the
    /// pricing table instead. See agent.TokenUsage.CostUSDTicks.
    #[serde(rename = "cost_usd_ticks", skip_serializing_if = "is_zero_i64")]
    pub cost_usd_ticks: i64,
}

/// TaskResult is the outcome of executing a task (types.go:280–303).
///
/// Go fields tagged `json:"-"` (EnvRoot, FailureReason,
/// SessionRolloutMissing, RetiredSessionID) are process-local only; they keep
/// their names but use `#[serde(skip)]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "comment")]
    pub comment: String,
    #[serde(rename = "branch_name", skip_serializing_if = "String::is_empty")]
    pub branch_name: String,
    #[serde(rename = "env_type", skip_serializing_if = "String::is_empty")]
    pub env_type: String,
    /// Claude session ID for future resumption.
    #[serde(rename = "session_id", skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// Working directory used during execution.
    #[serde(rename = "work_dir", skip_serializing_if = "String::is_empty")]
    pub work_dir: String,
    /// DurableWorkDir replaces WorkDir only after a disposable local worktree
    /// was finalized and its removal was confirmed. Empty keeps WorkDir
    /// authoritative.
    #[serde(rename = "durable_work_dir", skip_serializing_if = "String::is_empty")]
    pub durable_work_dir: String,
    /// Env root dir for writing GC metadata (not sent to server).
    #[serde(skip)]
    pub env_root: String,
    /// Classifier forwarded to FailTask on the blocked path; empty falls back
    /// to 'agent_error'.
    #[serde(skip)]
    pub failure_reason: String,
    /// SessionRolloutMissing is set when the daemon withheld this task's Codex
    /// session because its rollout was not in the store (PB-5305). Forwarded
    /// to the terminal report so the server clears the resume pointer and
    /// flags the continuity gap for the next claim. Not part of the wire
    /// result itself.
    #[serde(skip)]
    pub session_rollout_missing: bool,
    /// RetiredSessionID names a session this run was told to resume and then
    /// abandoned as unresumable (GH #6066). Forwarded on every terminal path,
    /// including the completed one: a fresh-session retry that SUCCEEDS is
    /// precisely when the abandoned id would otherwise stay selectable.
    #[serde(skip)]
    pub retired_session_id: String,
    /// Per-model token usage.
    #[serde(rename = "usage", skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<TaskUsageEntry>,
}

/// PluginHookTool is one agent-trigger plugin hook, as the agent will see it
/// (types.go:306–316).
///
/// Mirrors service.PluginHookTool on the wire. Declared here rather than
/// imported so the daemon does not depend on the server's service package —
/// same reason Task itself is a daemon-side type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginHookTool {
    #[serde(rename = "installation_id")]
    pub installation_id: String,
    #[serde(rename = "hook_key")]
    pub hook_key: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "description")]
    pub description: String,
    #[serde(rename = "input_schema", skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}
