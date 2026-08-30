//! Table models generated from the live schema (`information_schema`).

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Stable routing identity for one serialized execution lane.
///
/// The database stores the same value on every task row. Keep the constructor
/// in sync with the generated-column expression in migration 409: this value
/// is routing metadata, not provider transcript or prompt content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, sqlx::Type)]
#[sqlx(transparent)]
#[serde(transparent)]
pub struct ExecutionLaneKey(String);

impl ExecutionLaneKey {
    pub fn for_task(
        agent_id: Uuid,
        issue_id: Option<Uuid>,
        chat_session_id: Option<Uuid>,
        context: Option<&serde_json::Value>,
    ) -> Self {
        if let Some(chat_session_id) = chat_session_id {
            return Self(format!("chat:{chat_session_id}"));
        }

        let side_chat_key = context
            .and_then(serde_json::Value::as_object)
            .and_then(|context| {
                context
                    .get("side_chat_root_comment_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        context
                            .get("side_chat_parent_task_id")
                            .and_then(serde_json::Value::as_str)
                            .filter(|value| !value.is_empty())
                    })
            });

        match (issue_id, side_chat_key) {
            (Some(issue_id), Some(side_chat_key)) => Self(format!(
                "issue:{issue_id}:agent:{agent_id}:side:{side_chat_key}"
            )),
            (Some(issue_id), None) => Self(format!("issue:{issue_id}:agent:{agent_id}:main")),
            (None, _) => Self(format!("agent:{agent_id}:default")),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ExecutionLaneKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for ExecutionLaneKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod execution_lane_tests {
    use super::ExecutionLaneKey;
    use serde_json::json;
    use uuid::Uuid;

    const AGENT: Uuid = Uuid::from_u128(1);
    const ISSUE: Uuid = Uuid::from_u128(2);
    const CHAT: Uuid = Uuid::from_u128(3);

    #[test]
    fn execution_lane_key_covers_chat_issue_side_and_default() {
        assert_eq!(
            ExecutionLaneKey::for_task(AGENT, None, Some(CHAT), None).to_string(),
            format!("chat:{CHAT}")
        );
        assert_eq!(
            ExecutionLaneKey::for_task(AGENT, Some(ISSUE), None, None).to_string(),
            format!("issue:{ISSUE}:agent:{AGENT}:main")
        );
        assert_eq!(
            ExecutionLaneKey::for_task(
                AGENT,
                Some(ISSUE),
                None,
                Some(&json!({"side_chat_root_comment_id": "root-1"})),
            )
            .to_string(),
            format!("issue:{ISSUE}:agent:{AGENT}:side:root-1")
        );
        assert_eq!(
            ExecutionLaneKey::for_task(
                AGENT,
                Some(ISSUE),
                None,
                Some(&json!({"side_chat_parent_task_id": "parent-1"})),
            )
            .to_string(),
            format!("issue:{ISSUE}:agent:{AGENT}:side:parent-1")
        );
        assert_eq!(
            ExecutionLaneKey::for_task(AGENT, None, None, None).to_string(),
            format!("agent:{AGENT}:default")
        );
    }

    #[test]
    fn chat_lane_wins_over_issue_context_without_copying_context_content() {
        let context = json!({
            "side_chat_root_comment_id": "root-1",
            "internal_prompt": "must not become routing metadata",
        });

        assert_eq!(
            ExecutionLaneKey::for_task(AGENT, Some(ISSUE), Some(CHAT), Some(&context)).to_string(),
            format!("chat:{CHAT}")
        );
    }
}

/// Row of `activity_log`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActivityLog {
    pub action: String,
    pub actor_id: Option<Uuid>,
    pub actor_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub details: serde_json::Value,
    pub id: Uuid,
    pub issue_id: Option<Uuid>,
    pub workspace_id: Uuid,
}

/// Row of `agent`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Agent {
    pub archived_at: Option<DateTime<Utc>>,
    pub archived_by: Option<Uuid>,
    pub avatar_url: Option<String>,
    pub composio_toolkit_allowlist: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub custom_args: serde_json::Value,
    pub custom_env: serde_json::Value,
    pub description: String,
    pub disabled_runtime_skills: serde_json::Value,
    pub id: Uuid,
    pub instructions: String,
    pub kind: String,
    pub max_concurrent_tasks: i32,
    pub mcp_config: Option<serde_json::Value>,
    pub model: Option<String>,
    pub name: String,
    pub owner_id: Option<Uuid>,
    pub permission_mode: String,
    pub runtime_config: serde_json::Value,
    pub runtime_id: Option<Uuid>,
    pub runtime_mode: String,
    pub service_tier: Option<String>,
    pub status: String,
    pub system_key: Option<String>,
    pub thinking_level: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub visibility: String,
    pub workspace_id: Uuid,
}

/// Row of `agent_builder_draft`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentBuilderDraft {
    pub chat_session_id: Uuid,
    pub draft: serde_json::Value,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `agent_invocation_target`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentInvocationTarget {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub id: Uuid,
    pub target_id: Uuid,
    pub target_type: String,
}

/// Row of `agent_mcp_server`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentMcpServer {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub enabled: bool,
    pub server_id: Uuid,
}

/// Row of `agent_runtime`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentRuntime {
    pub created_at: DateTime<Utc>,
    pub custom_name: Option<String>,
    pub daemon_id: Option<String>,
    pub device_info: String,
    pub id: Uuid,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub legacy_daemon_id: Option<String>,
    pub metadata: serde_json::Value,
    pub name: String,
    pub owner_id: Option<Uuid>,
    pub profile_id: Option<Uuid>,
    pub provider: String,
    pub runtime_mode: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub visibility: String,
    pub workspace_id: Uuid,
}

/// Row of `agent_skill`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentSkill {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub enabled: bool,
    pub skill_id: Uuid,
}

/// Row of `agent_task_queue`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentTaskQueue {
    pub accountable_user_id: Option<Uuid>,
    pub agent_id: Uuid,
    pub attempt: i32,
    pub autopilot_run_id: Option<Uuid>,
    pub branch_name: Option<String>,
    pub chat_finalize_deferred_at: Option<DateTime<Utc>>,
    pub chat_input_task_id: Option<Uuid>,
    pub chat_session_id: Option<Uuid>,
    pub coalesced_comment_ids: Vec<Uuid>,
    pub completed_at: Option<DateTime<Utc>>,
    pub context: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub delegated_from_task_id: Option<Uuid>,
    pub delivered_comment_ids: Vec<Uuid>,
    pub dispatched_at: Option<DateTime<Utc>>,
    pub durable_work_dir: Option<String>,
    pub error: Option<String>,
    pub execution_lane_key: ExecutionLaneKey,
    pub escalation_for_task_id: Option<Uuid>,
    pub failure_reason: Option<String>,
    pub fire_at: Option<DateTime<Utc>>,
    pub force_fresh_session: bool,
    pub handoff_note: Option<String>,
    pub id: Uuid,
    pub initiator_user_id: Option<Uuid>,
    pub is_leader_task: bool,
    pub issue_id: Option<Uuid>,
    pub max_attempts: i32,
    pub originator_source: Option<String>,
    pub originator_user_id: Option<Uuid>,
    pub parent_task_id: Option<Uuid>,
    pub prepare_lease_expires_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub quick_actions_disabled: bool,
    pub regenerate_quick_actions_for: Option<Uuid>,
    pub rerun_of_task_id: Option<Uuid>,
    pub result: Option<serde_json::Value>,
    pub retired_session_id: Option<String>,
    pub retry_of_task_id: Option<Uuid>,
    pub rule_version_id: Option<Uuid>,
    pub runtime_connected_apps: Option<serde_json::Value>,
    pub runtime_id: Option<Uuid>,
    pub runtime_mcp_overlay: Option<serde_json::Value>,
    pub session_id: Option<String>,
    pub session_rollout_missing: bool,
    pub team_id: Option<Uuid>,
    pub started_at: Option<DateTime<Utc>>,
    pub status: String,
    pub trigger_comment_id: Option<Uuid>,
    pub trigger_evidence_kind: Option<String>,
    pub trigger_evidence_ref_id: Option<Uuid>,
    pub trigger_summary: Option<String>,
    pub wait_reason: Option<String>,
    pub work_dir: Option<String>,
}

/// Row of `agent_to_label`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentToLabel {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub label_id: Uuid,
}

/// Row of `attachment`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Attachment {
    pub chat_message_id: Option<Uuid>,
    pub chat_session_id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub content_type: String,
    pub created_at: DateTime<Utc>,
    pub filename: String,
    pub id: Uuid,
    pub issue_id: Option<Uuid>,
    pub size_bytes: i64,
    pub task_id: Option<Uuid>,
    pub uploader_id: Uuid,
    pub uploader_type: String,
    pub url: String,
    pub workspace_id: Uuid,
}

/// Row of `autopilot`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Autopilot {
    pub assignee_id: Uuid,
    pub assignee_type: String,
    pub created_at: DateTime<Utc>,
    pub created_by_id: Uuid,
    pub created_by_type: String,
    pub description: Option<String>,
    pub execution_mode: String,
    pub id: Uuid,
    pub issue_title_template: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub pause_reason: Option<String>,
    pub project_id: Option<Uuid>,
    pub status: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `autopilot_collaborator`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotCollaborator {
    pub autopilot_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub granted_by: Uuid,
    pub user_id: Uuid,
    pub user_type: String,
}

/// Row of `autopilot_quota_period`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotQuotaPeriod {
    pub blocked_counts: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub period_start: DateTime<Utc>,
    pub reserved_count: i64,
    pub updated_at: DateTime<Utc>,
    pub used_count: i64,
    pub workspace_id: Uuid,
}

/// Row of `autopilot_quota_reservation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotQuotaReservation {
    pub created_at: DateTime<Utc>,
    pub finalized_at: Option<DateTime<Utc>>,
    pub id: Uuid,
    pub idempotency_key: String,
    pub period_end: DateTime<Utc>,
    pub period_start: DateTime<Utc>,
    pub policy_revision: i64,
    pub source: String,
    pub state: String,
    pub subscription_version: i64,
    pub workspace_id: Uuid,
}

/// Row of `autopilot_rule_version`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotRuleVersion {
    pub autopilot_id: Uuid,
    pub config_summary: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub published_by_id: Option<Uuid>,
    pub published_by_type: String,
    pub workspace_id: Uuid,
}

/// Row of `autopilot_run`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotRun {
    pub autopilot_id: Uuid,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub failure_reason: Option<String>,
    pub id: Uuid,
    pub issue_id: Option<Uuid>,
    pub planned_at: Option<DateTime<Utc>>,
    pub quota_reservation_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub result: Option<serde_json::Value>,
    pub source: String,
    pub team_id: Option<Uuid>,
    pub status: String,
    pub task_id: Option<Uuid>,
    pub trigger_id: Option<Uuid>,
    pub trigger_payload: Option<serde_json::Value>,
    pub triggered_at: DateTime<Utc>,
    pub webhook_delivery_id: Option<Uuid>,
}

/// Row of `autopilot_subscriber`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotSubscriber {
    pub autopilot_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub user_type: String,
}

/// Row of `autopilot_trigger`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AutopilotTrigger {
    pub autopilot_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub cron_expression: Option<String>,
    pub enabled: bool,
    pub event_filters: Option<serde_json::Value>,
    pub id: Uuid,
    pub kind: String,
    pub label: Option<String>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub provider: String,
    pub published_by_id: Option<Uuid>,
    pub published_by_type: Option<String>,
    pub signing_secret: Option<String>,
    pub timezone: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub webhook_token: Option<String>,
}

/// Row of `channel_binding_token`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelBindingToken {
    pub channel_type: String,
    pub channel_user_id: String,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub installation_id: Uuid,
    pub token_hash: String,
    pub workspace_id: Uuid,
}

/// Row of `channel_chat_session_binding`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelChatSessionBinding {
    pub channel_chat_id: String,
    pub channel_type: String,
    pub chat_session_id: Uuid,
    pub chat_type: String,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub last_message_id: Option<String>,
    pub last_thread_id: Option<String>,
    pub pending_fresh: bool,
}

/// Row of `channel_inbound_audit`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelInboundAudit {
    pub channel_chat_id: Option<String>,
    pub channel_event_id: Option<String>,
    pub channel_message_id: Option<String>,
    pub channel_type: String,
    pub drop_reason: String,
    pub event_type: String,
    pub id: Uuid,
    pub installation_id: Option<Uuid>,
    pub received_at: DateTime<Utc>,
}

/// Row of `channel_inbound_message_dedup`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelInboundMessageDedup {
    pub claim_token: Uuid,
    pub installation_id: Uuid,
    pub message_id: String,
    pub processed_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

/// Row of `channel_installation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelInstallation {
    /// `None` means the platform is connected at workspace scope. The active
    /// Agent is selected per chat by the channel hub (`/agents`).
    pub agent_id: Option<Uuid>,
    pub channel_type: String,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installed_at: DateTime<Utc>,
    pub installer_user_id: Uuid,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
    pub ws_lease_expires_at: Option<DateTime<Utc>>,
    pub ws_lease_token: Option<String>,
}

/// Row of `channel_media_pending_object`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelMediaPendingObject {
    pub attempt: i32,
    pub chat_message_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub installation_id: Option<Uuid>,
    pub last_error: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub lease_token: Option<Uuid>,
    pub next_attempt_at: DateTime<Utc>,
    pub state: String,
    pub storage_key: String,
    pub storage_url: String,
    pub tombstone_pass: i32,
    pub workspace_id: Uuid,
}

/// Row of `channel_outbound_card_message`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelOutboundCardMessage {
    pub channel_card_message_id: String,
    pub channel_chat_id: String,
    pub channel_type: String,
    pub chat_session_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub last_patched_at: Option<DateTime<Utc>>,
    pub status: String,
    pub task_id: Option<Uuid>,
}

/// Row of `channel_user_binding`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChannelUserBinding {
    pub bound_at: DateTime<Utc>,
    pub channel_type: String,
    pub channel_user_id: String,
    pub config: serde_json::Value,
    pub patchbay_user_id: Uuid,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `chat_draft_restore`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChatDraftRestore {
    pub attachment_ids: Vec<Uuid>,
    pub chat_session_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub task_id: Uuid,
}

/// Row of `chat_message`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChatMessage {
    pub channel_ingested: bool,
    pub channel_media_pending_until: Option<DateTime<Utc>>,
    pub chat_session_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub elapsed_ms: Option<i64>,
    pub failure_reason: Option<String>,
    pub id: Uuid,
    pub message_kind: String,
    pub quick_actions: serde_json::Value,
    pub role: String,
    pub task_id: Option<Uuid>,
}

/// Row of `chat_pinned_agent`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChatPinnedAgent {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub position: f64,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `chat_session`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChatSession {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub creator_id: Uuid,
    pub id: Uuid,
    pub is_agent_intro: bool,
    pub last_read_at: DateTime<Utc>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub project_id: Option<Uuid>,
    pub runtime_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub status: String,
    pub title: String,
    pub unread_since: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub work_dir: Option<String>,
    pub workspace_id: Uuid,
}

/// Row of `workspace_channel`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkspaceChannel {
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub created_by: Uuid,
    pub description: String,
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// A quoted channel message with the author snapshot needed by the client.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceChannelQuotedMessage {
    pub author_id: Uuid,
    pub author_name: String,
    pub author_type: String,
    pub content: String,
    pub id: Uuid,
}

/// A channel message enriched with its member/agent display identity.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceChannelMessage {
    pub author_avatar_url: Option<String>,
    pub author_id: Uuid,
    pub author_name: String,
    pub author_status: Option<String>,
    pub author_type: String,
    pub channel_id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub parent_message: Option<WorkspaceChannelQuotedMessage>,
    pub parent_id: Option<Uuid>,
    pub quoted_message: Option<WorkspaceChannelQuotedMessage>,
    pub quoted_message_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `client_usage_daily`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ClientUsageDaily {
    pub activity_date: chrono::NaiveDate,
    pub client_type: String,
    pub client_version: String,
    pub created_at: DateTime<Utc>,
    pub first_active_at: DateTime<Utc>,
    pub install_id: Uuid,
    pub last_active_at: DateTime<Utc>,
    pub offline_count: Option<i32>,
    pub online_count: Option<i32>,
    pub os: String,
    pub probe_result: Option<String>,
    pub provider_summary: Option<serde_json::Value>,
    pub runtime_count: Option<i32>,
    pub runtime_probed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub workspace_id: Option<Uuid>,
}

/// Row of `comment`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Comment {
    pub author_id: Uuid,
    pub author_type: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub issue_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_id: Option<Uuid>,
    pub resolved_by_type: Option<String>,
    pub revision: i64,
    pub source_task_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub type_: String,
    pub updated_at: DateTime<Utc>,
    pub via_plugin_id: Option<Uuid>,
    pub workspace_id: Uuid,
}

/// Row of `comment_reaction`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CommentReaction {
    pub actor_id: Uuid,
    pub actor_type: String,
    pub comment_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub emoji: String,
    pub id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `contact_sales_inquiry`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ContactSalesInquiry {
    pub business_email: String,
    pub company_name: String,
    pub company_size: String,
    pub consent_outreach: bool,
    pub consent_updates: bool,
    pub country_region: String,
    pub created_at: DateTime<Utc>,
    pub first_name: String,
    pub goals: String,
    pub id: Uuid,
    pub last_name: String,
    pub submitter_ip: Option<String>,
    pub use_case: String,
    pub user_agent: String,
}

/// Row of `daemon_connection`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DaemonConnection {
    pub agent_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub daemon_id: String,
    pub id: Uuid,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub runtime_info: serde_json::Value,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

/// Row of `daemon_token`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DaemonToken {
    pub created_at: DateTime<Utc>,
    pub daemon_id: String,
    pub expires_at: DateTime<Utc>,
    pub id: Uuid,
    pub token_hash: String,
    pub workspace_id: Uuid,
}

/// Row of `dingtalk_group_route`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DingtalkGroupRoute {
    pub agent_id: Uuid,
    pub conversation_id: String,
    pub conversation_title: String,
    pub discovered_at: DateTime<Utc>,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub revision: i64,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `feedback`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Feedback {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub message: String,
    pub metadata: serde_json::Value,
    pub user_id: Uuid,
    pub workspace_id: Option<Uuid>,
}

/// Row of `github_installation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GithubInstallation {
    pub account_avatar_url: Option<String>,
    pub account_login: String,
    pub account_type: String,
    pub connected_by_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installation_id: i64,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `github_pending_check_suite`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GithubPendingCheckSuite {
    pub app_id: i64,
    pub conclusion: Option<String>,
    pub head_sha: String,
    pub installation_id: i64,
    pub pr_number: i32,
    pub received_at: DateTime<Utc>,
    pub repo_name: String,
    pub repo_owner: String,
    pub status: String,
    pub suite_id: i64,
    pub suite_updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `github_pending_installation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GithubPendingInstallation {
    pub account_avatar_url: Option<String>,
    pub account_login: String,
    pub account_type: String,
    pub installation_id: i64,
    pub received_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Row of `github_pull_request`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GithubPullRequest {
    pub additions: i32,
    pub api_merge_state_status: Option<String>,
    pub api_mergeable: Option<String>,
    pub author_avatar_url: Option<String>,
    pub author_login: Option<String>,
    pub branch: Option<String>,
    pub changed_files: i32,
    pub checks_rollup_state: Option<String>,
    pub closed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub deletions: i32,
    pub head_sha: String,
    pub html_url: String,
    pub id: Uuid,
    pub installation_id: Option<i64>,
    pub mergeable_state: Option<String>,
    pub merged_at: Option<DateTime<Utc>>,
    pub pr_created_at: DateTime<Utc>,
    pub pr_number: i32,
    pub pr_updated_at: DateTime<Utc>,
    pub repo_name: String,
    pub repo_owner: String,
    pub snapshot_fetched_at: Option<DateTime<Utc>>,
    pub snapshot_head_sha: String,
    pub state: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `github_pull_request_check_run`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GithubPullRequestCheckRun {
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
    pub head_sha: String,
    pub is_status_context: bool,
    pub name: String,
    pub ordinal: i32,
    pub pr_id: Uuid,
    pub status: String,
}

/// Row of `github_pull_request_check_suite`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GithubPullRequestCheckSuite {
    pub app_id: i64,
    pub conclusion: Option<String>,
    pub head_sha: String,
    pub pr_id: Uuid,
    pub status: String,
    pub suite_id: i64,
    pub updated_at: DateTime<Utc>,
}

/// Row of `inbox_item`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InboxItem {
    pub actor_id: Option<Uuid>,
    pub actor_type: Option<String>,
    pub archived: bool,
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
    pub details: Option<serde_json::Value>,
    pub id: Uuid,
    pub issue_id: Option<Uuid>,
    pub read: bool,
    pub recipient_id: Uuid,
    pub recipient_type: String,
    pub severity: String,
    pub title: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub workspace_id: Uuid,
}

/// Row of `issue`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Issue {
    pub acceptance_criteria: serde_json::Value,
    pub assignee_id: Option<Uuid>,
    pub assignee_type: Option<String>,
    pub context_refs: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub creator_id: Uuid,
    pub creator_type: String,
    pub description: Option<String>,
    pub due_date: Option<chrono::NaiveDate>,
    pub first_executed_at: Option<DateTime<Utc>>,
    pub id: Uuid,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub metadata: serde_json::Value,
    pub number: i32,
    pub origin_id: Option<Uuid>,
    pub origin_type: Option<String>,
    pub parent_issue_id: Option<Uuid>,
    pub position: f64,
    pub priority: String,
    pub project_id: Option<Uuid>,
    pub properties: serde_json::Value,
    pub revision: i64,
    pub reviewer_id: Option<Uuid>,
    pub reviewer_type: Option<String>,
    pub stage: Option<i32>,
    pub start_date: Option<chrono::NaiveDate>,
    pub status: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `issue_dependency`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueDependency {
    pub depends_on_issue_id: Uuid,
    pub id: Uuid,
    pub issue_id: Uuid,
    #[serde(rename = "type")]
    pub type_: String,
}

/// A validated, immutable planner submission for one parent issue.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DependencyGraphPlan {
    pub attention_reason: Option<String>,
    pub attention_required: bool,
    pub created_at: DateTime<Utc>,
    pub created_by_id: Uuid,
    pub created_by_type: String,
    pub goal: String,
    pub id: Uuid,
    pub idempotency_key: String,
    pub parent_issue_id: Uuid,
    pub request_hash: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// A planner node and the issue allocated for it by atomic plan application.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DependencyGraphNode {
    pub acceptance_criteria: serde_json::Value,
    pub assignee_id: Option<Uuid>,
    pub assignee_type: Option<String>,
    pub candidate_assignees: serde_json::Value,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub description: String,
    pub id: Uuid,
    pub issue_id: Uuid,
    pub outputs: serde_json::Value,
    pub plan_id: Uuid,
    pub temp_id: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub wave: i32,
    pub workspace_id: Uuid,
}

/// A directed hard edge. Direction is always prerequisite (`from`) to
/// dependent (`to`); it intentionally does not reuse issue_dependency's
/// bidirectional `blocks`/`blocked_by` vocabulary.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DependencyGraphEdge {
    pub consumed_output: String,
    pub created_at: DateTime<Utc>,
    pub from_issue_id: Uuid,
    pub id: Uuid,
    pub plan_id: Uuid,
    pub reason: String,
    pub to_issue_id: Uuid,
    #[serde(rename = "type")]
    pub type_: String,
    pub workspace_id: Uuid,
}

/// Row of `issue_label`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueLabel {
    pub color: String,
    pub created_at: DateTime<Utc>,
    pub description: String,
    pub id: Uuid,
    pub name: String,
    pub resource_type: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `issue_property`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueProperty {
    pub archived_at: Option<DateTime<Utc>>,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub description: String,
    pub icon: String,
    pub id: Uuid,
    pub name: String,
    pub position: f64,
    #[serde(rename = "type")]
    pub type_: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `issue_reaction`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueReaction {
    pub actor_id: Uuid,
    pub actor_type: String,
    pub created_at: DateTime<Utc>,
    pub emoji: String,
    pub id: Uuid,
    pub issue_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `issue_status`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueStatus {
    pub archived_at: Option<DateTime<Utc>>,
    pub category: String,
    pub color: String,
    pub created_at: DateTime<Utc>,
    pub description: String,
    pub id: Uuid,
    pub is_system: bool,
    pub key: String,
    pub name: String,
    pub position: f64,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `issue_subscriber`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueSubscriber {
    pub created_at: DateTime<Utc>,
    pub issue_id: Uuid,
    pub opt_out_scope: Option<String>,
    pub reason: String,
    pub unsubscribed_at: Option<DateTime<Utc>>,
    pub user_id: Uuid,
    pub user_type: String,
}

/// Row of `issue_to_label`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueToLabel {
    pub issue_id: Uuid,
    pub label_id: Uuid,
}

/// Row of `issue_view`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueView {
    pub created_at: DateTime<Utc>,
    pub definition_version: i32,
    pub display: serde_json::Value,
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub query: serde_json::Value,
    pub revision: i32,
    pub scope_id: Option<Uuid>,
    pub scope_type: String,
    pub scope_variant: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub visibility: String,
    pub workspace_id: Uuid,
}

/// Row of `issue_view_preference`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IssueViewPreference {
    pub prefs: serde_json::Value,
    pub scope_id: Uuid,
    pub scope_type: String,
    pub updated_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `lark_binding_token`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkBindingToken {
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub installation_id: Uuid,
    pub lark_open_id: String,
    pub token_hash: String,
    pub workspace_id: Uuid,
}

/// Row of `lark_chat_session_binding`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkChatSessionBinding {
    pub chat_session_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub lark_chat_id: String,
    pub lark_chat_type: String,
    pub last_lark_message_id: Option<String>,
    pub last_lark_thread_id: Option<String>,
}

/// Row of `lark_inbound_audit`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkInboundAudit {
    pub drop_reason: String,
    pub event_type: String,
    pub id: Uuid,
    pub installation_id: Option<Uuid>,
    pub lark_chat_id: Option<String>,
    pub lark_event_id: Option<String>,
    pub lark_message_id: Option<String>,
    pub received_at: DateTime<Utc>,
}

/// Row of `lark_inbound_message_dedup`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkInboundMessageDedup {
    pub claim_token: Uuid,
    pub installation_id: Uuid,
    pub message_id: String,
    pub processed_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

/// Row of `lark_installation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkInstallation {
    pub agent_id: Uuid,
    pub app_id: String,
    pub app_secret_encrypted: Vec<u8>,
    pub bot_open_id: String,
    pub bot_union_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installed_at: DateTime<Utc>,
    pub installer_user_id: Uuid,
    pub region: String,
    pub status: String,
    pub tenant_key: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
    pub ws_lease_expires_at: Option<DateTime<Utc>>,
    pub ws_lease_token: Option<String>,
}

/// Row of `lark_outbound_card_message`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkOutboundCardMessage {
    pub chat_session_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub lark_card_message_id: String,
    pub lark_chat_id: String,
    pub last_patched_at: Option<DateTime<Utc>>,
    pub status: String,
    pub task_id: Option<Uuid>,
}

/// Row of `lark_user_binding`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LarkUserBinding {
    pub bound_at: DateTime<Utc>,
    pub patchbay_user_id: Uuid,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub lark_open_id: String,
    pub union_id: Option<String>,
    pub workspace_id: Uuid,
}

/// Row of `member`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Member {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub role: String,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `notification_preference`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct NotificationPreference {
    pub id: Uuid,
    pub preferences: serde_json::Value,
    pub updated_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `personal_access_token`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PersonalAccessToken {
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub id: Uuid,
    pub last_used_at: Option<DateTime<Utc>>,
    pub name: String,
    pub revoked: bool,
    pub token_hash: String,
    pub token_prefix: String,
    pub user_id: Uuid,
}

/// Row of `pinned_item`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PinnedItem {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub item_id: Uuid,
    pub item_type: String,
    pub position: f64,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `plugin_installation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PluginInstallation {
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub enabled: bool,
    pub granted_scopes: serde_json::Value,
    pub id: Uuid,
    pub installed_by: Option<Uuid>,
    pub manifest: serde_json::Value,
    pub mcp_approvals: serde_json::Value,
    pub plugin_key: String,
    pub source_url: String,
    pub token_hash: Option<String>,
    pub token_rotated_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub version: String,
    pub workspace_id: Uuid,
}

/// Row of `plugin_invocation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PluginInvocation {
    pub attempt: i32,
    pub created_at: DateTime<Utc>,
    pub error: Option<String>,
    pub event_type: Option<String>,
    pub hook_key: String,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub latency_ms: i32,
    pub status: String,
    pub trigger: String,
    pub workspace_id: Uuid,
}

/// Row of `plugin_secret`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PluginSecret {
    pub ciphertext: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub key: String,
    pub updated_at: DateTime<Utc>,
}

/// Row of `plugin_storage`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PluginStorage {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub installation_id: Uuid,
    pub key: String,
    pub scope_id: Uuid,
    pub scope_type: String,
    pub updated_at: DateTime<Utc>,
    pub value: String,
}

/// Row of `project`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Project {
    pub created_at: DateTime<Utc>,
    pub description: Option<String>,
    pub due_date: Option<chrono::NaiveDate>,
    pub icon: Option<String>,
    pub id: Uuid,
    pub lead_id: Option<Uuid>,
    pub lead_type: Option<String>,
    pub priority: String,
    pub start_date: Option<chrono::NaiveDate>,
    pub status: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `project_resource`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProjectResource {
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub id: Uuid,
    pub label: Option<String>,
    pub position: i32,
    pub project_id: Uuid,
    pub resource_ref: serde_json::Value,
    pub resource_type: String,
    pub workspace_id: Uuid,
}

/// Row of `quick_action`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct QuickAction {
    pub assignee_id: Uuid,
    pub assignee_type: String,
    pub created_at: DateTime<Utc>,
    pub created_by_id: Uuid,
    pub created_by_type: String,
    pub description: String,
    pub id: Uuid,
    pub last_used_at: Option<DateTime<Utc>>,
    pub name: String,
    pub prompt: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub use_count: i64,
    pub visibility: String,
    pub workspace_id: Uuid,
}

/// Row of `runtime_profile`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RuntimeProfile {
    pub command_name: String,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub description: Option<String>,
    pub display_name: String,
    pub enabled: bool,
    pub fixed_args: serde_json::Value,
    pub id: Uuid,
    pub protocol_family: String,
    pub updated_at: DateTime<Utc>,
    pub visibility: String,
    pub workspace_id: Uuid,
}

/// Row of `schema_migrations`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SchemaMigrations {
    pub applied_at: DateTime<Utc>,
    pub version: String,
}

/// Row of `skill`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Skill {
    pub config: serde_json::Value,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub description: String,
    pub id: Uuid,
    pub name: String,
    pub plugin_installation_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `skill_file`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SkillFile {
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub path: String,
    pub skill_id: Uuid,
    pub updated_at: DateTime<Utc>,
}

/// Row of `skill_to_label`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SkillToLabel {
    pub created_at: DateTime<Utc>,
    pub label_id: Uuid,
    pub skill_id: Uuid,
}

/// Row of `team`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Team {
    pub archived_at: Option<DateTime<Utc>>,
    pub archived_by: Option<Uuid>,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub creator_id: Uuid,
    pub description: String,
    pub id: Uuid,
    pub instructions: String,
    pub leader_id: Uuid,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `team_member`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TeamMember {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub member_id: Uuid,
    pub member_type: String,
    pub role: String,
    pub team_id: Uuid,
}

/// Row of `sys_cron_executions`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SysCronExecutions {
    pub attempt: i32,
    pub created_at: DateTime<Utc>,
    pub duration_ms: Option<i32>,
    pub error_code: Option<String>,
    pub error_msg: Option<String>,
    pub finished_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub id: Uuid,
    pub job_name: String,
    pub lease_token: Uuid,
    pub max_attempts: i32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub plan_time: DateTime<Utc>,
    pub result: serde_json::Value,
    pub rows_affected: Option<i64>,
    pub runner_id: Option<String>,
    pub scope_id: String,
    pub scope_kind: String,
    pub stale_after: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

/// Row of `task_message`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskMessage {
    pub content: Option<String>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub input: Option<serde_json::Value>,
    pub output: Option<String>,
    pub seq: i32,
    pub task_id: Uuid,
    pub tool: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
}

/// Row of `task_token`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskToken {
    pub agent_id: Uuid,
    pub claim_dispatched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub delegation_depth: i32,
    pub delegation_fence: i64,
    pub device_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub id: Uuid,
    pub on_behalf_of_user_id: Option<Uuid>,
    pub parent_fence: Option<i64>,
    pub parent_token_id: Option<Uuid>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_reason: Option<String>,
    pub scope: serde_json::Value,
    pub task_id: Uuid,
    pub token_hash: String,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `task_usage`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskUsage {
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd_ticks: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub input_tokens: i64,
    pub model: String,
    pub output_tokens: i64,
    pub provider: String,
    pub task_id: Uuid,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Row of `task_usage_hourly`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskUsageHourly {
    pub agent_id: Uuid,
    pub bucket_hour: DateTime<Utc>,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd_ticks: i64,
    pub event_count: i64,
    pub input_tokens: i64,
    pub model: String,
    pub output_tokens: i64,
    pub project_id: Option<Uuid>,
    pub provider: String,
    pub runtime_id: Uuid,
    pub task_count: i64,
    pub uncosted_cache_read_tokens: Option<i64>,
    pub uncosted_cache_write_tokens: Option<i64>,
    pub uncosted_input_tokens: Option<i64>,
    pub uncosted_output_tokens: Option<i64>,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `task_usage_hourly_dirty`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskUsageHourlyDirty {
    pub agent_id: Uuid,
    pub bucket_hour: DateTime<Utc>,
    pub enqueued_at: DateTime<Utc>,
    pub model: String,
    pub project_id: Option<Uuid>,
    pub provider: String,
    pub runtime_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `task_usage_hourly_rollup_state`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TaskUsageHourlyRollupState {
    pub id: i16,
    pub last_error: Option<String>,
    pub last_run_finished_at: Option<DateTime<Utc>>,
    pub last_run_rows: i64,
    pub last_run_started_at: Option<DateTime<Utc>>,
    pub watermark_at: DateTime<Utc>,
}

/// Row of `user`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub avatar_url: Option<String>,
    pub cloud_waitlist_email: Option<String>,
    pub cloud_waitlist_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub email: String,
    pub id: Uuid,
    /// True for a server-backed guest account. Guest users retain a normal
    /// UUID and real API permissions, but cannot perform formal-account-only
    /// operations such as external authorization or billing.
    pub is_guest: bool,
    pub language: Option<String>,
    pub name: String,
    pub onboarded_at: Option<DateTime<Utc>>,
    pub onboarding_questionnaire: serde_json::Value,
    pub profile_description: String,
    pub starter_content_state: Option<String>,
    pub timezone: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Row of `user_composio_connection`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UserComposioConnection {
    pub auth_config_id: String,
    pub composio_user_id: String,
    pub connected_account_id: String,
    pub connected_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub last_used_at: Option<DateTime<Utc>>,
    pub status: String,
    pub toolkit_slug: String,
    pub updated_at: DateTime<Utc>,
    pub user_id: Uuid,
}

/// Row of `vcs_commit_status`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VcsCommitStatus {
    pub connection_id: Uuid,
    pub context: String,
    pub description: Option<String>,
    pub sha: String,
    pub state: String,
    pub target_url: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Row of `vcs_connection`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VcsConnection {
    pub access_token_encrypted: String,
    pub account_login: String,
    pub connected_by_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
    pub instance_url: String,
    pub provider: String,
    pub updated_at: DateTime<Utc>,
    pub webhook_secret_encrypted: String,
    pub workspace_id: Uuid,
}

/// Row of `vcs_pull_request`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VcsPullRequest {
    pub additions: i32,
    pub author_avatar_url: Option<String>,
    pub author_login: Option<String>,
    pub branch: Option<String>,
    pub changed_files: i32,
    pub closed_at: Option<DateTime<Utc>>,
    pub connection_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub deletions: i32,
    pub head_sha: String,
    pub html_url: String,
    pub id: Uuid,
    pub merged_at: Option<DateTime<Utc>>,
    pub pr_created_at: DateTime<Utc>,
    pub pr_number: i32,
    pub pr_updated_at: DateTime<Utc>,
    pub provider: String,
    pub repo_name: String,
    pub repo_owner: String,
    pub state: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `work_product`.
///
/// Work products are the provider-neutral identity for externally hosted
/// artifacts. Provider mirrors (for example `github_pull_request`) remain
/// responsible for snapshots; this row is the identity that relations attach
/// to.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkProduct {
    pub created_at: DateTime<Utc>,
    pub external_identity: String,
    pub external_url: Option<String>,
    pub id: Uuid,
    pub kind: String,
    pub provider: String,
    pub provider_record_id: Option<Uuid>,
    pub provider_record_type: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `work_product_relation`.
///
/// `relation_source` is one of the three canonical, auditable paths: a manual
/// user attach, a task execution attach, or a unique exact-branch discovery
/// performed from that task's persisted execution provenance. A relation never
/// records a guessed text match, and the nullable task/run fields are filled by
/// the server from authenticated context rather than request JSON.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkProductRelation {
    pub attached_at: DateTime<Utc>,
    pub attached_by_id: Uuid,
    pub attached_by_type: String,
    pub close_intent: bool,
    pub detached_at: Option<DateTime<Utc>>,
    pub detached_by_id: Option<Uuid>,
    pub detached_by_type: Option<String>,
    pub detached_run_id: Option<Uuid>,
    pub detached_task_id: Option<Uuid>,
    pub id: Uuid,
    pub issue_id: Option<Uuid>,
    pub relation_key: String,
    pub relation_source: String,
    pub run_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub work_product_id: Uuid,
    pub workspace_id: Uuid,
}

/// Server-persisted execution provenance used by the post-run branch
/// discovery path. One task may have multiple rows, one per exact repository
/// checkout/workspace key. The task id is the ownership boundary: callers can
/// only write rows through the authenticated daemon/task context, and the
/// discovery result is an audit of that exact execution rather than a
/// reusable branch association.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentTaskExecutionProvenance {
    pub task_id: Uuid,
    pub workspace_id: Uuid,
    pub run_id: Option<Uuid>,
    pub repo_identity: Option<String>,
    pub execution_workspace: Option<String>,
    pub head_branch: Option<String>,
    pub head_sha: Option<String>,
    pub head_state: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub discovery_status: String,
    pub discovery_match_count: i32,
    pub discovery_reason: Option<String>,
    pub discovery_work_product_id: Option<Uuid>,
    pub discovery_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Row of `verification_code`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct VerificationCode {
    pub attempts: i32,
    pub code: String,
    pub created_at: DateTime<Utc>,
    pub email: String,
    pub expires_at: DateTime<Utc>,
    pub id: Uuid,
    pub used: bool,
}

/// Row of `webhook_delivery`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WebhookDelivery {
    pub attempt_count: i32,
    pub autopilot_id: Uuid,
    pub autopilot_run_id: Option<Uuid>,
    pub available_at: DateTime<Utc>,
    pub content_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub dedupe_key: Option<String>,
    pub dedupe_source: Option<String>,
    pub dispatch_attempts: i32,
    pub error: Option<String>,
    pub event: String,
    pub id: Uuid,
    pub last_attempt_at: DateTime<Utc>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub lease_token: Option<Uuid>,
    pub provider: String,
    pub raw_body: Option<Vec<u8>>,
    pub reason_code: Option<String>,
    pub received_at: DateTime<Utc>,
    pub replay_idempotency_key: Option<String>,
    pub replayed_from_delivery_id: Option<Uuid>,
    pub response_body: Option<String>,
    pub response_status: Option<i32>,
    pub selected_headers: serde_json::Value,
    pub signature_status: String,
    pub status: String,
    pub trigger_id: Uuid,
    pub workspace_id: Uuid,
}

/// Row of `workspace`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Workspace {
    pub attribution_fail_closed: bool,
    pub avatar_url: Option<String>,
    pub context: Option<String>,
    pub created_at: DateTime<Utc>,
    pub description: Option<String>,
    pub id: Uuid,
    pub issue_counter: i32,
    pub issue_prefix: String,
    pub name: String,
    pub repos: serde_json::Value,
    pub settings: serde_json::Value,
    pub slug: String,
    pub updated_at: DateTime<Utc>,
}

/// Row of `workspace_invitation`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkspaceInvitation {
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub id: Uuid,
    pub invitee_email: String,
    pub invitee_user_id: Option<Uuid>,
    pub inviter_id: Uuid,
    pub role: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `workspace_mcp_server`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkspaceMcpServer {
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub id: Uuid,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

/// Row of `workspace_share_link`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkspaceShareLink {
    pub code: String,
    pub created_at: DateTime<Utc>,
    pub created_by: Uuid,
    pub expires_at: Option<DateTime<Utc>>,
    pub id: Uuid,
    pub is_active: bool,
    pub max_uses: Option<i32>,
    pub role: String,
    pub use_count: i32,
    pub workspace_id: Uuid,
}

#[cfg(test)]
mod wire_compatibility_tests {
    use super::*;
    use serde::Serialize;

    fn assert_type_field<T: Serialize>(value: &T) -> serde_json::Value {
        let json = serde_json::to_value(value).expect("model serializes");
        let object = json.as_object().expect("model is an object");
        assert!(
            object.get("type").is_some(),
            "Go wire key `type` is present"
        );
        assert!(
            object.get("type_").is_none(),
            "Rust field name must not leak"
        );
        json
    }

    #[test]
    fn reserved_type_fields_keep_go_wire_key() {
        let id = Uuid::nil();
        let now = Utc::now();

        assert_type_field(&Comment {
            author_id: id,
            author_type: "member".into(),
            content: "body".into(),
            created_at: now,
            id,
            issue_id: id,
            parent_id: None,
            quick_action_id: None,
            resolved_at: None,
            resolved_by_id: None,
            resolved_by_type: None,
            revision: 1,
            source_task_id: None,
            type_: "comment".into(),
            updated_at: now,
            via_plugin_id: None,
            workspace_id: id,
        });

        let inbox = assert_type_field(&InboxItem {
            actor_id: None,
            actor_type: None,
            archived: false,
            body: None,
            created_at: now,
            details: None,
            id,
            issue_id: None,
            read: false,
            recipient_id: id,
            recipient_type: "member".into(),
            severity: "info".into(),
            title: "title".into(),
            type_: "issue".into(),
            workspace_id: id,
        });
        assert!(inbox.get("details").is_some_and(serde_json::Value::is_null));

        assert_type_field(&IssueDependency {
            depends_on_issue_id: id,
            id,
            issue_id: id,
            type_: "blocks".into(),
        });

        assert_type_field(&IssueProperty {
            archived_at: None,
            config: serde_json::json!({}),
            created_at: now,
            description: "description".into(),
            icon: "".into(),
            id,
            name: "priority".into(),
            position: 0.0,
            type_: "text".into(),
            updated_at: now,
            workspace_id: id,
        });

        let message = assert_type_field(&TaskMessage {
            content: None,
            created_at: now,
            id,
            input: None,
            output: None,
            seq: 1,
            task_id: id,
            tool: None,
            type_: "assistant".into(),
        });
        assert!(message.get("input").is_some_and(serde_json::Value::is_null));
    }
}
