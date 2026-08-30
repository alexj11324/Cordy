//! WebSocket message envelope and payload structs.
//!
//! Field names and `omitempty` semantics match the Go json tags byte-level:
//! value-type omitempty elides empty strings and zero ints; pointer omitempty
//! elides nil.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- Capability negotiation constants ------------------------------------

pub const DAEMON_CAPABILITY_SKILL_BUNDLES_V1: &str = "skill-bundles-v1";
pub const DAEMON_CAPABILITY_COALESCED_COMMENTS_V1: &str = "coalesced-comments-v1";
pub const DAEMON_CAPABILITY_EXECUTION_MANIFEST_V1: &str = "execution-manifest-v1";
pub const DAEMON_CAPABILITY_AGENT_SKILL_V1: &str = "agent-skill-v1";
pub const DAEMON_CAPABILITY_REMOTE_MCP_V1: &str = "remote-mcp-v1";
/// Advertises that the daemon implements worktree mode for local_directory
/// resources (execution_mode=worktree). A CAPABILITY rather than a version
/// check on purpose: a daemon without the implementation json-skips
/// execution_mode and runs the task IN PLACE, editing the working copy the
/// user asked to isolate (PB-5707).
pub const DAEMON_CAPABILITY_LOCAL_WORKTREE_V1: &str = "local-worktree-v1";
/// Advertises that the daemon can carry request/response RPCs over the
/// WebSocket control connection (PB-4257).
pub const DAEMON_CAPABILITY_RPC_V1: &str = "rpc-v1";
/// Advertised (X-Client-Capabilities) by app clients that understand the
/// durable draft-restore recovery path (#5219).
pub const APP_CAPABILITY_CHAT_DRAFT_RESTORE_V1: &str = "chat-draft-restore-v1";

// --- Chat message kinds ---------------------------------------------------

/// Ordinary user/assistant message.
pub const CHAT_MESSAGE_KIND_MESSAGE: &str = "message";
/// A direct-chat turn the agent completed without any text reply — a visible,
/// deliberate terminal outcome rather than a silently-dropped turn (PB-4351).
pub const CHAT_MESSAGE_KIND_NO_RESPONSE: &str = "no_response";
/// The server-authored, hidden first turn used to start Mika's onboarding
/// conversation. User-facing APIs filter it out.
pub const CHAT_MESSAGE_KIND_ONBOARDING_KICKOFF: &str = "onboarding_kickoff";
/// The assistant reply produced by the onboarding kickoff; chat renders the
/// starter cards under this kind instead of quick-action chips (PB-5765).
pub const CHAT_MESSAGE_KIND_ONBOARDING_OPENING: &str = "onboarding_opening";

// --- Pending work kinds ----------------------------------------------------

/// Advisory only — the daemon reacts identically to every kind, so an unknown
/// value from a newer server stays safe on an older daemon.
pub const PENDING_WORK_KIND_MODEL_LIST: &str = "model_list";

// --- Chat cancel outcomes ---------------------------------------------------

/// The Agent event history turned out non-empty, so a "Stopped." assistant message was
/// persisted.
pub const CHAT_CANCEL_OUTCOME_STOPPED: &str = "stopped";
/// The Agent event history stayed empty, so the triggering user message was deleted and
/// its content should be restored into the composer as a draft.
pub const CHAT_CANCEL_OUTCOME_RESTORED: &str = "restored";

/// The ack Status used when the runtime row no longer exists server-side.
pub const HEARTBEAT_STATUS_RUNTIME_GONE: &str = "runtime_gone";

// --- omitempty helpers ------------------------------------------------------

fn is_zero<T: PartialEq + Default>(n: &T) -> bool {
    *n == T::default()
}

fn serialize_double_option<S>(v: &Option<Option<String>>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match v {
        Some(inner) => inner.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

fn deserialize_double_option<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

/// Server-validated follow-up attached to one assistant reply. Label is the
/// concise chip text; Prompt is the full next user turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatQuickAction {
    pub label: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary: bool,
}

/// Generic daemon→server request envelope carried in a [`Message`] of type
/// EVENT_DAEMON_RPC_REQUEST. RequestID correlates the response; Method selects
/// the server-side handler. TimeoutMs bounds the handler's context server-side
/// so a slow RPC is cancelled rather than committing after the daemon has
/// already timed out waiting (PB-4257); 0 means connection-lifetime only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequestPayload {
    #[serde(rename = "request_id")]
    pub request_id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, skip_serializing_if = "is_zero", rename = "timeout_ms")]
    pub timeout_ms: i64,
}

/// Server→daemon reply carried in a [`Message`] of type
/// EVENT_DAEMON_RPC_RESPONSE. Status mirrors an HTTP status so the daemon can
/// treat WS and HTTP outcomes uniformly. Exactly one of Body / Error is
/// meaningful.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponsePayload {
    #[serde(rename = "request_id")]
    pub request_id: String,
    pub status: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// The envelope for all WebSocket messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "type")]
    pub r#type: String,
    pub payload: Value,
}

/// Sent from server to daemon when a task is assigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDispatchPayload {
    #[serde(rename = "task_id")]
    pub task_id: String,
    #[serde(rename = "issue_id")]
    pub issue_id: String,
    pub title: String,
    pub description: String,
}

/// Sent from server to daemon as a wakeup hint. The daemon still claims work
/// through the existing HTTP claim endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskAvailablePayload {
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "task_id")]
    pub task_id: String,
}

/// Sent from server to daemon when a workspace custom runtime profile is
/// created, edited, disabled, or deleted. The daemon still fetches profiles
/// through the existing HTTP endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeProfilesChangedPayload {
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "runtime_profile_id"
    )]
    pub runtime_profile_id: String,
}

/// An account-scoped hint that asks a daemon to reconcile its workspace
/// membership set. The server remains authoritative; no workspace data is
/// embedded in the event.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorkspacesChangedPayload {}

/// Sent from server to daemon when a heartbeat-carried request is enqueued for
/// a runtime. Carries no work itself — safe to lose, duplicate, or ignore
/// (PB-5444).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingWorkPayload {
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// Sent from daemon to server during task execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskProgressPayload {
    #[serde(rename = "task_id")]
    pub task_id: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub step: i32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub total: i32,
}

/// Sent from daemon to server when a task finishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskCompletedPayload {
    #[serde(rename = "task_id")]
    pub task_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// Supplements one completed chat turn with the sanitized follow-up actions
/// from the suggestion pass. An empty QuickActions list is a meaningful
/// terminal state — it resolves the pending skeleton with "no suggestions
/// this turn".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatQuickActionsPayload {
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
    #[serde(rename = "task_id")]
    pub task_id: String,
    #[serde(rename = "message_id")]
    pub message_id: String,
    #[serde(rename = "quick_actions")]
    pub quick_actions: Vec<ChatQuickAction>,
    /// Marks a supplement that resolves the client's refresh spinner because
    /// the regeneration FAILED, not because it produced new suggestions
    /// (PB-5149). Omitted on the success path and for the automatic pass.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

/// A single agent execution message (tool call, text, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskMessagePayload {
    #[serde(rename = "task_id")]
    pub task_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "issue_id")]
    pub issue_id: String,
    pub seq: i32,
    /// "text", "tool_use", "tool_result", "error"
    #[serde(rename = "type")]
    pub r#type: String,
    /// Tool name for tool_use/tool_result.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// Text content.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// Tool input (tool_use only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, Value>>,
    /// Tool output (tool_result only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "created_at"
    )]
    pub created_at: String,
}

/// Sent from daemon to server on connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonRegisterPayload {
    #[serde(rename = "daemon_id")]
    pub daemon_id: String,
    #[serde(rename = "agent_id")]
    pub agent_id: String,
    pub runtimes: Vec<RuntimeInfo>,
}

/// Describes an available agent runtime on the daemon's machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    #[serde(rename = "type")]
    pub r#type: String,
    pub version: String,
    pub status: String,
}

/// Broadcast when a new chat message is created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessagePayload {
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
    #[serde(rename = "message_id")]
    pub message_id: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "task_id")]
    pub task_id: String,
    #[serde(rename = "created_at")]
    pub created_at: String,
}

/// Broadcast when an agent finishes responding to a chat message. Carries the
/// freshly-persisted assistant ChatMessage so the client can write it into the
/// messages cache inline (#2123). MessageKind is additive (PB-4351): older
/// clients ignore it and fall back to the non-empty Content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatDonePayload {
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
    #[serde(rename = "task_id")]
    pub task_id: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "message_id"
    )]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "is_zero", rename = "elapsed_ms")]
    pub elapsed_ms: i64,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "created_at"
    )]
    pub created_at: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "message_kind"
    )]
    pub message_kind: String,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "quick_actions"
    )]
    pub quick_actions: Vec<ChatQuickAction>,
    /// Tells clients a chat:quick_actions supplement will follow for this turn
    /// (render a placeholder). Never true when QuickActions is populated.
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        rename = "quick_actions_pending"
    )]
    pub quick_actions_pending: bool,
}

/// Broadcast when a cancelled chat task's deferred finalization settles
/// (#5219). Outcome "stopped" inserts the assistant message; outcome
/// "restored" removes the deleted user message from caches and prompts the
/// initiator's client to fetch the durable draft restore. The restored
/// prompt's content and attachments deliberately never ride this
/// workspace-wide broadcast.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatCancelFinalizedPayload {
    pub outcome: String,
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
    #[serde(rename = "task_id")]
    pub task_id: String,
    /// The human who triggered the cancelled task. Only this user's client
    /// needs to fetch the draft restore; clients treat a missing value as
    /// "not me".
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "initiator_user_id"
    )]
    pub initiator_user_id: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "message_id"
    )]
    pub message_id: String,
    /// Describe the persisted "Stopped." assistant row; set only for outcome
    /// "stopped" — the same exposure surface as chat:done.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "message_kind"
    )]
    pub message_kind: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "created_at"
    )]
    pub created_at: String,
    #[serde(default, skip_serializing_if = "is_zero", rename = "elapsed_ms")]
    pub elapsed_ms: i64,
}

/// Broadcast when the creator marks a session as read; fires to other devices
/// so their unread counts stay in sync.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSessionReadPayload {
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
}

/// Broadcast when a chat session is hard-deleted so other tabs/devices drop it
/// from their session lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSessionDeletedPayload {
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
}

/// Broadcast when a user-editable field on a chat session changes (today:
/// title via inline rename). Other tabs/devices patch the session row in their
/// cached list without a full refetch.
///
/// `project_id` mirrors Go's **string double pointer: absent (field omitted)
/// means "not touched by this update", explicit null means "detach from the
/// project". `pinned`/`status` are plain pointers: None leaves the receiver's
/// state untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSessionUpdatedPayload {
    #[serde(rename = "chat_session_id")]
    pub chat_session_id: String,
    pub title: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_double_option",
        deserialize_with = "deserialize_double_option",
        rename = "project_id"
    )]
    pub project_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "updated_at")]
    pub updated_at: String,
}

/// Sent from daemon to server over WebSocket to update last_seen_at and pull
/// pending actions for a single runtime. Mirrors POST /api/daemon/heartbeat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonHeartbeatRequestPayload {
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        rename = "supports_batch_import"
    )]
    pub supports_batch_import: bool,
}

/// The server's reply to [`DaemonHeartbeatRequestPayload`]. JSON shape mirrors
/// the HTTP heartbeat response so daemon code can decode either.
/// RuntimeGone is the WS replacement for the HTTP 404 "runtime not found":
/// the daemon prunes the stale runtime and re-registers instead of heartbeating
/// a dead UUID until process restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonHeartbeatAckPayload {
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    pub status: String,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "server_capabilities"
    )]
    pub server_capabilities: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        rename = "runtime_gone"
    )]
    pub runtime_gone: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pending_update"
    )]
    pub pending_update: Option<DaemonHeartbeatPendingUpdate>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pending_model_list"
    )]
    pub pending_model_list: Option<DaemonHeartbeatPendingModelList>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pending_local_skills"
    )]
    pub pending_local_skills: Option<DaemonHeartbeatPendingLocalSkills>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pending_local_skill_import"
    )]
    pub pending_local_skill_import: Option<DaemonHeartbeatPendingLocalSkillImport>,
    /// Multiple import requests in one heartbeat so the daemon can process
    /// them concurrently. Old daemons silently ignore it and fall back to the
    /// singular field above.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "pending_local_skill_imports"
    )]
    pub pending_local_skill_imports: Vec<DaemonHeartbeatPendingLocalSkillImport>,
}

/// Describes a CLI-update action the daemon should run for the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonHeartbeatPendingUpdate {
    pub id: String,
    #[serde(rename = "target_version")]
    pub target_version: String,
}

/// Requests the daemon enumerate the runtime's supported models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonHeartbeatPendingModelList {
    pub id: String,
}

/// Requests the runtime's local-skill inventory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonHeartbeatPendingLocalSkills {
    pub id: String,
}

/// Requests import of a specific runtime local skill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonHeartbeatPendingLocalSkillImport {
    pub id: String,
    #[serde(rename = "skill_key")]
    pub skill_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_wire_values_are_byte_stable() {
        assert_eq!(CHAT_MESSAGE_KIND_MESSAGE, "message");
        assert_eq!(CHAT_MESSAGE_KIND_NO_RESPONSE, "no_response");
        assert_eq!(CHAT_MESSAGE_KIND_ONBOARDING_KICKOFF, "onboarding_kickoff");
        assert_eq!(CHAT_MESSAGE_KIND_ONBOARDING_OPENING, "onboarding_opening");
        assert_eq!(PENDING_WORK_KIND_MODEL_LIST, "model_list");
        assert_eq!(CHAT_CANCEL_OUTCOME_STOPPED, "stopped");
        assert_eq!(CHAT_CANCEL_OUTCOME_RESTORED, "restored");
        assert_eq!(HEARTBEAT_STATUS_RUNTIME_GONE, "runtime_gone");
        assert_eq!(DAEMON_CAPABILITY_LOCAL_WORKTREE_V1, "local-worktree-v1");
        assert_eq!(
            APP_CAPABILITY_CHAT_DRAFT_RESTORE_V1,
            "chat-draft-restore-v1"
        );
    }

    #[test]
    fn chat_quick_action_omitempty_primary() {
        let plain = ChatQuickAction {
            label: "a".into(),
            prompt: "b".into(),
            primary: false,
        };
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"label":"a","prompt":"b"}"#
        );
        let primary = ChatQuickAction {
            label: "a".into(),
            prompt: "b".into(),
            primary: true,
        };
        assert_eq!(
            serde_json::to_string(&primary).unwrap(),
            r#"{"label":"a","prompt":"b","primary":true}"#
        );
    }

    #[test]
    fn chat_done_elides_all_optional_fields_when_zero() {
        let p = ChatDonePayload {
            chat_session_id: "cs1".into(),
            task_id: "t1".into(),
            message_id: "".into(),
            content: "".into(),
            elapsed_ms: 0,
            created_at: "".into(),
            message_kind: "".into(),
            quick_actions: vec![],
            quick_actions_pending: false,
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"chat_session_id":"cs1","task_id":"t1"}"#
        );
    }

    #[test]
    fn chat_quick_actions_payload_always_carries_list() {
        let p = ChatQuickActionsPayload {
            chat_session_id: "cs".into(),
            task_id: "t".into(),
            message_id: "m".into(),
            quick_actions: vec![],
            failed: false,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""quick_actions":[]"#), "{s}");
        assert!(
            !s.contains("failed"),
            "omitempty failed must be elided: {s}"
        );
    }

    #[test]
    fn session_updated_double_pointer_semantics() {
        // Plain rename: project_id/pinned/status all absent.
        let rename = ChatSessionUpdatedPayload {
            chat_session_id: "cs".into(),
            title: "New".into(),
            project_id: None,
            pinned: None,
            status: None,
            updated_at: "t".into(),
        };
        assert_eq!(
            serde_json::to_string(&rename).unwrap(),
            r#"{"chat_session_id":"cs","title":"New","updated_at":"t"}"#
        );

        // Explicit null detach vs value set.
        let detach = ChatSessionUpdatedPayload {
            chat_session_id: "cs".into(),
            title: "N".into(),
            project_id: Some(None),
            pinned: Some(false),
            status: Some("archived".into()),
            updated_at: "t".into(),
        };
        let s = serde_json::to_string(&detach).unwrap();
        assert!(s.contains(r#""project_id":null"#), "{s}");
        assert!(s.contains(r#""pinned":false"#), "{s}");
        assert!(s.contains(r#""status":"archived""#), "{s}");

        // Roundtrip preserves the three-way distinction.
        let parsed: ChatSessionUpdatedPayload = serde_json::from_str(
            r#"{"chat_session_id":"c","title":"x","project_id":null,"updated_at":"u"}"#,
        )
        .unwrap();
        assert_eq!(parsed.project_id, Some(None));
        assert_eq!(parsed.pinned, None);
        let parsed: ChatSessionUpdatedPayload = serde_json::from_str(
            r#"{"chat_session_id":"c","title":"x","project_id":"p1","updated_at":"u"}"#,
        )
        .unwrap();
        assert_eq!(parsed.project_id, Some(Some("p1".to_string())));
        // Absent field stays None (untouched), not null.
        let parsed: ChatSessionUpdatedPayload =
            serde_json::from_str(r#"{"chat_session_id":"c","title":"x","updated_at":"u"}"#)
                .unwrap();
        assert_eq!(parsed.project_id, None);
    }

    #[test]
    fn heartbeat_ack_nested_optionals_and_gone_flag() {
        let ack = DaemonHeartbeatAckPayload {
            runtime_id: "rt".into(),
            status: HEARTBEAT_STATUS_RUNTIME_GONE.into(),
            server_capabilities: vec![],
            runtime_gone: true,
            pending_update: None,
            pending_model_list: None,
            pending_local_skills: None,
            pending_local_skill_import: None,
            pending_local_skill_imports: vec![],
        };
        let s = serde_json::to_string(&ack).unwrap();
        assert!(s.contains(r#""status":"runtime_gone""#), "{s}");
        assert!(s.contains(r#""runtime_gone":true"#), "{s}");
        assert!(!s.contains("pending_update"), "{s}");
        assert!(!s.contains("server_capabilities"), "{s}");

        // Full shape roundtrips.
        let full = DaemonHeartbeatAckPayload {
            runtime_id: "rt".into(),
            status: "ok".into(),
            server_capabilities: vec![DAEMON_CAPABILITY_RPC_V1.into()],
            runtime_gone: false,
            pending_update: Some(DaemonHeartbeatPendingUpdate {
                id: "u1".into(),
                target_version: "v2".into(),
            }),
            pending_model_list: Some(DaemonHeartbeatPendingModelList { id: "m1".into() }),
            pending_local_skills: Some(DaemonHeartbeatPendingLocalSkills { id: "s1".into() }),
            pending_local_skill_import: Some(DaemonHeartbeatPendingLocalSkillImport {
                id: "i1".into(),
                skill_key: "k".into(),
            }),
            pending_local_skill_imports: vec![DaemonHeartbeatPendingLocalSkillImport {
                id: "i2".into(),
                skill_key: "k2".into(),
            }],
        };
        let back: DaemonHeartbeatAckPayload =
            serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn rpc_envelope_roundtrip() {
        let msg = Message {
            r#type: "daemon:rpc_request".into(),
            payload: serde_json::json!({
                "request_id": "r1",
                "method": "tasks.claim",
                "body": {"runtime_id": "rt"},
                "timeout_ms": 5000
            }),
        };
        let s = serde_json::to_string(&msg).unwrap();
        assert!(s.starts_with(r#"{"type":"daemon:rpc_request","#), "{s}");
        let req: RpcRequestPayload = serde_json::from_value(msg.payload.clone()).unwrap();
        assert_eq!(req.request_id, "r1");
        assert_eq!(req.method, "tasks.claim");
        assert_eq!(req.timeout_ms, 5000);

        // Zero timeout omitted.
        let bare = RpcRequestPayload {
            request_id: "r".into(),
            method: "m".into(),
            body: None,
            timeout_ms: 0,
        };
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"request_id":"r","method":"m"}"#
        );
    }

    #[test]
    fn task_message_type_field_renamed() {
        let tm = TaskMessagePayload {
            task_id: "t".into(),
            issue_id: "".into(),
            seq: 1,
            r#type: "tool_use".into(),
            tool: "bash".into(),
            content: "".into(),
            input: None,
            output: "".into(),
            created_at: "".into(),
        };
        let s = serde_json::to_string(&tm).unwrap();
        assert!(s.contains(r#""type":"tool_use""#), "{s}");
        assert!(!s.contains("issue_id"), "{s}");
        assert!(!s.contains("input"), "{s}");
    }
}
