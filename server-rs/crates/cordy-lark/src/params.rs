//! Domain parameter types for the channel-backed Feishu store — port of
//! `server/internal/integrations/lark/params.go`.
//!
//! They replace the retired db.*LarkParams shapes generated from
//! queries/lark.sql, using the same channel-neutral field names as the
//! domain entities in [`crate::store`]. The store ([`crate::channel_store`])
//! maps them onto the channel_* writes, folding the feishu-specific
//! identifiers into the JSONB config at the DB boundary.
//!
//! Port note: Go's pgtype.UUID → `uuid::Uuid`, pgtype.Text → `Option<String>`,
//! pgtype.Timestamptz → `Option<chrono::DateTime<Utc>>`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Scopes an installation lookup to a workspace.
#[derive(Debug, Clone)]
pub struct GetInstallationInWorkspaceParams {
    pub id: Uuid,
    pub workspace_id: Uuid,
}

/// Carries the flat feishu installation fields for an install / re-install.
#[derive(Debug, Clone)]
pub struct UpsertInstallationParams {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub app_id: String,
    pub app_secret_encrypted: Vec<u8>,
    pub bot_open_id: String,
    pub installer_user_id: Uuid,
    pub tenant_key: Option<String>,
    pub bot_union_id: Option<String>,
    pub region: String,
}

/// Flips an installation's status (active/revoked).
#[derive(Debug, Clone)]
pub struct SetInstallationStatusParams {
    pub id: Uuid,
    pub status: String,
}

/// Records the bot's union_id (backfill).
#[derive(Debug, Clone)]
pub struct SetInstallationBotUnionIdParams {
    pub id: Uuid,
    pub bot_union_id: Option<String>,
}

/// Fences the WS supervisor lease for an installation.
#[derive(Debug, Clone)]
pub struct AcquireWsLeaseParams {
    pub new_token: Option<String>,
    pub new_expires_at: Option<DateTime<Utc>>,
    pub id: Uuid,
}

/// Releases a WS supervisor lease the caller still holds.
#[derive(Debug, Clone)]
pub struct ReleaseWsLeaseParams {
    pub id: Uuid,
    pub current_token: Option<String>,
}

/// Looks up a binding by its channel-native user id.
#[derive(Debug, Clone)]
pub struct GetUserBindingByOpenIdParams {
    pub installation_id: Uuid,
    pub channel_user_id: String,
}

/// Binds a workspace member to a channel-native user id.
#[derive(Debug, Clone)]
pub struct CreateUserBindingParams {
    pub workspace_id: Uuid,
    pub cordy_user_id: Uuid,
    pub installation_id: Uuid,
    pub channel_user_id: String,
    pub union_id: Option<String>,
}

/// Looks up a chat binding by its channel chat id.
#[derive(Debug, Clone)]
pub struct GetChatSessionBindingParams {
    pub installation_id: Uuid,
    pub channel_chat_id: String,
}

/// Records the latest inbound trigger message + thread so the outbound
/// patcher can thread its reply.
#[derive(Debug, Clone)]
pub struct UpdateChatSessionBindingReplyTargetParams {
    pub chat_session_id: Uuid,
    pub last_message_id: Option<String>,
    pub last_thread_id: Option<String>,
}

/// Claims the two-phase idempotency row for a message.
#[derive(Debug, Clone)]
pub struct ClaimInboundDedupParams {
    pub installation_id: Uuid,
    pub message_id: String,
}

/// Marks a claimed message processed (fenced).
#[derive(Debug, Clone)]
pub struct MarkInboundDedupProcessedParams {
    pub installation_id: Uuid,
    pub message_id: String,
    pub claim_token: Uuid,
}

/// Releases a claim on processing failure (fenced).
#[derive(Debug, Clone)]
pub struct ReleaseInboundDedupParams {
    pub installation_id: Uuid,
    pub message_id: String,
    pub claim_token: Uuid,
}

/// Writes a non-content drop audit row.
#[derive(Debug, Clone, Default)]
pub struct RecordInboundDropParams {
    pub event_type: String,
    pub drop_reason: String,
    /// None preserves Go's invalid pgtype.UUID as SQL NULL for an
    /// installation-less event.
    pub installation_id: Option<Uuid>,
    pub channel_chat_id: Option<String>,
    pub channel_event_id: Option<String>,
    pub channel_message_id: Option<String>,
}

/// Mints a short-lived channel binding token.
#[derive(Debug, Clone)]
pub struct CreateBindingTokenParams {
    pub token_hash: String,
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    pub channel_user_id: String,
    pub expires_at: DateTime<Utc>,
}

/// Records an outbound card for a task/session.
#[derive(Debug, Clone)]
pub struct CreateOutboundCardMessageParams {
    pub chat_session_id: Uuid,
    pub channel_chat_id: String,
    pub channel_card_message_id: String,
    pub status: String,
    pub task_id: Option<Uuid>,
}

/// Transitions an outbound card's status.
#[derive(Debug, Clone)]
pub struct UpdateOutboundCardStatusParams {
    pub id: Uuid,
    pub status: String,
}
