//! Inbound drop-audit seam.
//!
//! The chat-session ensure/append/`/issue` machinery that used to live in the
//! Go chat.go has moved to the channel-agnostic engine ChatSession
//! ([`patchbay_channel_engine::session`]), which Feishu consumes via the shared
//! resolver set — there is no Feishu-specific session service anymore. What
//! remains is the inbound drop audit seam, still Feishu-shaped.

use async_trait::async_trait;
use uuid::Uuid;

use crate::types::ChatId;
use crate::types::DropReason;

/// Records dropped inbound events to channel_inbound_audit. The interface
/// deliberately does not accept a message body — see the drop-audit policy in
/// PB-2671 §4.7.
#[async_trait]
pub trait AuditLogger: Send + Sync {
    async fn record_drop(&self, p: AuditDropParams);
}

#[derive(Debug, Clone, Default)]
pub struct AuditDropParams {
    /// The nil UUID stands in for Go's invalid pgtype.UUID (an
    /// installation-less event).
    pub installation_id: Uuid,
    pub chat_id: ChatId,
    pub event_type: String,
    pub lark_event_id: String,
    pub lark_message_id: String,
    pub reason: DropReason,
}
