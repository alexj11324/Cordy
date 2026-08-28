//! Domain value types.
//!
//! Open string newtypes mirror the Go aliases so unknown future wire
//! values round-trip without a schema change.

use std::time::Duration;

/// A Lark user's per-installation identifier. Different installations of
/// the same app produce different open_ids for the same human user;
/// cross-installation identity merging would need union_id (Phase 2).
/// Typed alias instead of plain String so callers can't accidentally pass
/// a Cordy user UUID where a Lark open_id is expected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct OpenId(pub String);

impl OpenId {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for OpenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifies a Lark conversation (p2p or group). One ChatID maps to one
/// Cordy chat_session via channel_chat_session_binding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ChatId(pub String);

impl ChatId {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for ChatId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Discriminates p2p (single-user DM with the Bot) from group chats. The
/// DB column constraint channel_chat_session_binding.chat_type carries the
/// same two values.
///
/// Port note: Go defines its own `lark.ChatType`; Rust reuses the shared
/// `cordy_channel::ChatType` open newtype and exposes the two constructors
/// under lark-flavored names to keep call sites readable.
pub type ChatType = cordy_channel::message::ChatType;

/// The p2p chat-type wire value.
pub fn chat_type_p2p() -> ChatType {
    ChatType::p2p()
}

/// The group chat-type wire value.
pub fn chat_type_group() -> ChatType {
    ChatType::group()
}

/// Mirrors the channel_installation status values used by this adapter. A
/// revoked installation accepts no further events; its WebSocket is torn
/// down and inbound events are dropped with an audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationStatus(pub String);

impl InstallationStatus {
    pub const ACTIVE: &'static str = "active";
    pub const REVOKED: &'static str = "revoked";

    pub fn active() -> Self {
        Self(Self::ACTIVE.to_string())
    }
    pub fn revoked() -> Self {
        Self(Self::REVOKED.to_string())
    }

    pub fn is_active(&self) -> bool {
        self.0 == Self::ACTIVE
    }
}

/// Identifies which Lark open-platform cloud an installation lives on.
/// Feishu (mainland China, open.feishu.cn / accounts.feishu.cn) and Lark
/// (international, open.larksuite.com / accounts.larksuite.com) are
/// separate clouds with distinct hosts; a single Cordy deployment serves
/// both by resolving the host per installation from this value rather than
/// from a deployment-wide env var. Mirrors the region value folded into
/// the channel_installation config JSON — keep the two in lockstep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Region {
    /// Mainland 飞书 cloud (open.feishu.cn). The default every pre-region
    /// installation row carries.
    #[default]
    Feishu,
    /// International Lark cloud (open.larksuite.com).
    Lark,
}

impl Region {
    /// The mainland 飞书 open-platform host. It doubles as the WS
    /// long-conn bootstrap host (the /callback/ws/endpoint POST runs
    /// against the same open-platform host).
    pub const FEISHU_OPEN_BASE_URL: &'static str = "https://open.feishu.cn";
    /// The open-platform host for the Lark international cloud.
    pub const LARK_INTERNATIONAL_OPEN_BASE_URL: &'static str = "https://open.larksuite.com";

    /// Maps a region to its open-platform host — the base URL for both
    /// the REST API ([`crate::http_client`]) and the WebSocket
    /// /callback/ws/endpoint bootstrap ([`crate::ws_endpoint`]).
    pub fn open_platform_base_url(self) -> &'static str {
        match self {
            Region::Lark => Self::LARK_INTERNATIONAL_OPEN_BASE_URL,
            Region::Feishu => Self::FEISHU_OPEN_BASE_URL,
        }
    }

    /// Renders the stored region string (originating from the
    /// installation config's `region` key).
    pub fn as_str(self) -> &'static str {
        match self {
            Region::Feishu => "feishu",
            Region::Lark => "lark",
        }
    }
}

/// Normalizes a stored region string (originating from the installation
/// config) to a [`Region`], defaulting to Feishu for empty or unrecognized
/// values so a malformed row never resolves to an empty host (or a
/// CHECK-violating write). Exported because the router's WS credentials
/// provider hydrates creds from the raw row.
pub fn region_or_default(s: &str) -> Region {
    if s == "lark" {
        return Region::Lark;
    }
    Region::Feishu
}

/// DropReason enumerates the categories the inbound pipeline writes into
/// channel_inbound_audit.drop_reason. The DB column is open TEXT so new
/// reasons can be added without a migration; callers should reuse these
/// constants to keep dashboards / queries consistent.
///
/// All drop_reason values are recorded WITHOUT message body — see
/// MUL-2671 §4.7 (drop-audit policy).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DropReason(pub String);

impl DropReason {
    /// The sender's open_id has no binding row for this installation. The
    /// Bot replies with the binding card; the message itself is not stored.
    pub fn unbound_user() -> Self {
        Self("unbound_user".to_string())
    }
    /// The sender resolved to a Cordy user, but that user is not a member
    /// of this installation's workspace. The Bot replies with a "not in
    /// this workspace" notice; the message itself is not stored.
    pub fn non_workspace_member() -> Self {
        Self("non_workspace_member".to_string())
    }
    /// The message arrived in a group chat but did not @ the Bot and was
    /// not a reply to a Bot card. Group chats only ingest messages
    /// explicitly addressed to the Bot.
    pub fn not_addressed_in_group() -> Self {
        Self("not_addressed_in_group".to_string())
    }
    /// message_id already present in the inbound dedup table. WebSocket
    /// reconnects can replay events; this is the idempotency path.
    pub fn duplicate() -> Self {
        Self("duplicate".to_string())
    }
    /// installation.status='revoked'. The WS connection should already be
    /// closed; this catches any in-flight events that landed during
    /// teardown.
    pub fn revoked_installation() -> Self {
        Self("revoked_installation".to_string())
    }
    /// Payload failed schema validation (missing required fields, wrong
    /// event_type for this hook, etc.).
    pub fn invalid_event() -> Self {
        Self("invalid_event".to_string())
    }
}

impl std::fmt::Display for DropReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Caps the lifetime of a member-binding token. The DB CHECK on
/// channel_binding_token (`expires_at <= created_at + INTERVAL '15
/// minutes'`) enforces the same bound at the storage layer, so a
/// misconfigured caller or a hand-inserted SQL row cannot exceed it.
/// Keep these two values in sync if the product value changes.
pub const BINDING_TOKEN_TTL: Duration = Duration::from_secs(15 * 60);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_defaults_to_feishu() {
        assert_eq!(region_or_default(""), Region::Feishu);
        assert_eq!(region_or_default("feishu"), Region::Feishu);
        assert_eq!(region_or_default("bogus"), Region::Feishu);
        assert_eq!(region_or_default("lark"), Region::Lark);
    }

    #[test]
    fn region_resolves_open_platform_hosts() {
        assert_eq!(
            Region::Feishu.open_platform_base_url(),
            "https://open.feishu.cn"
        );
        assert_eq!(
            Region::Lark.open_platform_base_url(),
            "https://open.larksuite.com"
        );
    }

    #[test]
    fn drop_reason_wire_values_match_go_constants() {
        assert_eq!(DropReason::unbound_user().0, "unbound_user");
        assert_eq!(DropReason::non_workspace_member().0, "non_workspace_member");
        assert_eq!(
            DropReason::not_addressed_in_group().0,
            "not_addressed_in_group"
        );
        assert_eq!(DropReason::duplicate().0, "duplicate");
        assert_eq!(DropReason::revoked_installation().0, "revoked_installation");
        assert_eq!(DropReason::invalid_event().0, "invalid_event");
    }

    #[test]
    fn binding_token_ttl_is_fifteen_minutes() {
        assert_eq!(BINDING_TOKEN_TTL, Duration::from_secs(900));
    }
}
