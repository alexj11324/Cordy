//! Normalized cross-platform message envelopes.
//!
//! Every
//! adapter translates its platform's raw payload into an
//! [`InboundMessage`]; the core's router, dedup, identity check, and
//! persistence read ONLY these fields. Per the boundary rule (PB-3515
//! §2) the struct holds only cross-platform-true fields; everything
//! platform-specific lives in [`InboundMessage::raw`].

use serde_json::Value;

/// Discriminates a 1:1 direct conversation with the bot from a
/// multi-party group chat.
///
/// Port note: Go uses string constants. Rust keeps the same open-string
/// semantics with a newtype whose wire values match the existing
/// lark_chat_session_binding.lark_chat_type constraint so the generalized
/// channel_* table backfills 1:1. Unknown future values round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ChatType(pub String);

impl ChatType {
    /// A direct (peer-to-peer) conversation with the bot.
    pub fn p2p() -> ChatType {
        ChatType("p2p".to_string())
    }
    /// A multi-party group conversation.
    pub fn group() -> ChatType {
        ChatType("group".to_string())
    }
}

impl std::fmt::Display for ChatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The normalized, cross-platform message kind. Adapters map their
/// platform's native type onto this small closed set; the platform's raw
/// type string (Lark "post" / "merge_forward" / "interactive", …) is NOT
/// represented here — it stays in [`InboundMessage::raw`] and is read
/// only by the adapter. The core only ever needs to know "text vs media,
/// and which media".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct MsgType(pub String);

impl MsgType {
    /// A plain or rich text message. The human-readable content is
    /// flattened into [`InboundMessage::text`] by the adapter.
    pub fn text() -> MsgType {
        MsgType("text".to_string())
    }
    /// An image attachment.
    pub fn image() -> MsgType {
        MsgType("image".to_string())
    }
    /// A generic file attachment.
    pub fn file() -> MsgType {
        MsgType("file".to_string())
    }
    /// A voice / audio attachment.
    pub fn audio() -> MsgType {
        MsgType("audio".to_string())
    }
    /// A video attachment.
    pub fn video() -> MsgType {
        MsgType("video".to_string())
    }
    /// The fallback for a platform type the adapter does not map. The
    /// core treats it as a non-text, non-actionable message.
    pub fn unknown() -> MsgType {
        MsgType("unknown".to_string())
    }

    /// The closed set of normalized kinds, for classification helpers.
    pub fn is_media(&self) -> bool {
        matches!(self.0.as_str(), "image" | "file" | "audio" | "video")
    }
}

impl std::fmt::Display for MsgType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Carries the cross-platform routing identity of an inbound message —
/// every field here is true on every platform. Platform-specific routing
/// keys (a Lark app_id, a Slack team id) are resolved to an installation
/// by the adapter and do NOT appear on `Source`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Source {
    /// The platform the message arrived on; equals the owning Channel's
    /// type.
    pub channel_type: crate::channel::Type,

    /// The platform conversation identifier. One `chat_id` maps to one
    /// Cordy chat_session via the channel_chat_session_binding.
    pub chat_id: String,

    /// Discriminates direct from group conversations.
    pub chat_type: ChatType,

    /// The platform-native, per-installation user identifier (Lark
    /// open_id, Slack user id, …). It is stable WITHIN one installation
    /// and is the key the identity binding is stored under. It is NOT
    /// comparable across installations.
    pub sender_id: String,

    /// The platform's cross-installation stable identity for the sender
    /// when one exists (Lark union_id, …), otherwise empty. Captured
    /// opportunistically for future cross-installation identity merging;
    /// the core treats an empty value as "not available".
    pub sender_stable_id: String,

    /// The platform thread / topic the message belongs to, when threading
    /// applies and the message is inside a thread. Empty means a
    /// top-level conversation message. The core persists it so a
    /// decoupled outbound reply can be threaded back into the same topic.
    pub thread_id: String,
}

/// References a media attachment that the adapter has ALREADY persisted
/// to object storage before the message reaches the core. The core never
/// holds raw bytes — only this reference — so the envelope stays small
/// and platform-neutral.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaRef {
    /// The normalized media kind (image / file / audio / video).
    pub r#type: MsgType,
    /// Locates the persisted object in Cordy object storage.
    pub storage_key: String,
    /// The object URL returned by the storage backend and persisted on
    /// the attachment row so the existing attachment download endpoints
    /// can re-open it later.
    pub storage_url: String,
    /// The original display name, when the platform supplies one.
    pub filename: String,
    /// The content type, when known.
    pub mime_type: String,
    /// The object size in bytes, or 0 when unknown.
    pub size_bytes: i64,
    /// An optional exact marker in the durable message body that this
    /// attachment should replace with a stable Markdown link. Empty keeps
    /// the attachment standalone and preserves existing platform
    /// behavior. `inline_index` is the zero-based occurrence of that
    /// marker, so a partial media failure cannot shift later attachments
    /// into the wrong place.
    pub inline_placeholder: String,
    pub inline_index: usize,
}

/// Describes the message an inbound message quotes / replies to. `None`
/// (the Go nil pointer) when the inbound message is not a reply.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReplyCtx {
    /// The immediate parent message's platform id (the message being
    /// quoted).
    pub message_id: String,
    /// The thread/root anchor the platform reports, when any.
    pub root_id: String,
}

/// The single normalized shape the core consumes. Every adapter
/// translates its platform's raw payload into this struct; the core's
/// router, dedup, identity check, and persistence read ONLY these fields.
#[derive(Debug, Clone, Default)]
pub struct InboundMessage {
    /// The platform's delivery/event identifier; together with
    /// [`Self::message_id`] this backs the idempotency layer: a platform
    /// may redeliver the same event on reconnect, and dedup keys on
    /// (installation, message_id).
    pub event_id: String,
    /// The platform's message identifier (see [`Self::event_id`]).
    pub message_id: String,

    /// The routing identity (chat, sender, thread).
    pub source: Source,

    /// The normalized message kind.
    pub r#type: MsgType,

    /// The agent-readable content, flattened by the adapter. Router or an
    /// adapter may strip a command directive or enrich it with quoted
    /// context. For non-text messages it may be empty or a short
    /// placeholder; the media itself is in [`Self::media_refs`].
    pub text: String,

    /// The user's normalized text before command stripping or contextual
    /// enrichment. Shared command classifiers read this field so a
    /// rewritten text is never interpreted as a second command. Empty
    /// means "use text".
    pub command_text: String,

    /// The OUTPUT channel of the engine's media resolver: the objects it
    /// downloaded and uploaded for this message, each covered by an
    /// intent-ledger row written before its PUT. Inbound messages always
    /// arrive with this EMPTY — adapters must not pre-populate it,
    /// because binding only attaches refs whose ledger intent it can
    /// claim.
    pub media_refs: Vec<MediaRef>,

    /// The quoted/replied-to context, or `None`.
    pub reply_to: Option<ReplyCtx>,

    /// The adapter's normalized verdict on whether a GROUP message is an
    /// interaction with the bot (@-mention or reply to a bot message). It
    /// is meaningless for direct (p2p) chats and the core ignores it
    /// there.
    pub addressed_to_bot: bool,

    /// Asks the core to start a fresh agent session for this message
    /// instead of resuming the prior one. Router recognizes the shared
    /// /new text command; adapters may also set this flag for a native
    /// platform affordance.
    pub force_fresh: bool,

    /// Asks the core to persist this message + create any engine-side
    /// artefacts (issue from /issue command, session binding row) but NOT
    /// trigger an agent run afterwards. Set by an adapter when the
    /// message is a pure control command whose only meaningful effect is
    /// the artefact (wecom uses it for standalone /issue invocations).
    /// Left unset where the current cross-platform behavior — /issue
    /// triggers the agent as a normal chat turn — should be preserved
    /// (Feishu, Slack today).
    pub skip_agent_run: bool,

    /// The untouched platform payload. Adapters stash platform-specific
    /// fields here (Lark raw msg_type / parent_id / root_id / mention
    /// arrays, …) and read them back only inside the adapter. The core
    /// never reads this — that is the whole point of the boundary.
    ///
    /// Port note: Go holds `json.RawMessage`; Rust holds the decoded
    /// `serde_json::Value` (`Null` ≈ absent) because every producer in
    /// this workspace already decodes payloads for field extraction.
    pub raw: Value,
}

/// The minimal outbound reply the core can ask any Channel to deliver: a
/// text body into a chat, optionally threaded or quoting a specific
/// message. Rich cards, media uploads, and outbound webhooks are
/// deliberately NOT modeled here (PB-3515 decision §6) — an adapter that
/// supports richer output exposes it on its own type, not on this
/// cross-platform envelope.
#[derive(Debug, Clone, Default)]
pub struct OutboundMessage {
    /// The destination conversation (the platform chat id).
    pub chat_id: String,
    /// The message body.
    pub text: String,
    /// When set, threads the reply into the given platform thread /
    /// topic. Empty sends at the chat level.
    pub thread_id: String,
    /// When set, quote-replies to the given platform message id.
    pub reply_to: String,
}

/// The outcome of [`crate::Channel::send`].
#[derive(Debug, Clone, Default)]
pub struct SendResult {
    /// The platform's identifier for the delivered message.
    pub message_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_type_wire_values_match_go_constants() {
        assert_eq!(ChatType::p2p().0, "p2p");
        assert_eq!(ChatType::group().0, "group");
        assert_eq!(MsgType::text().0, "text");
        assert_eq!(MsgType::image().0, "image");
        assert_eq!(MsgType::file().0, "file");
        assert_eq!(MsgType::audio().0, "audio");
        assert_eq!(MsgType::video().0, "video");
        assert_eq!(MsgType::unknown().0, "unknown");
    }

    #[test]
    fn msg_type_media_classification() {
        for m in [
            MsgType::image(),
            MsgType::file(),
            MsgType::audio(),
            MsgType::video(),
        ] {
            assert!(m.is_media(), "{m} is media");
        }
        assert!(!MsgType::text().is_media());
        assert!(!MsgType::unknown().is_media());
    }

    #[test]
    fn default_inbound_is_empty_envelope() {
        let msg = InboundMessage::default();
        // Media refs arrive empty per contract: binding only attaches
        // refs whose ledger intent it can claim.
        assert!(msg.media_refs.is_empty());
        assert!(msg.reply_to.is_none());
        assert!(!msg.addressed_to_bot);
        assert!(!msg.force_fresh);
        assert!(!msg.skip_agent_run);
        assert!(msg.raw.is_null());
    }

    #[test]
    fn open_string_types_roundtrip_unknown_values() {
        // A future platform value must survive untouched (open-set
        // semantics, mirroring Go string types).
        let src = Source {
            chat_type: ChatType("channel".to_string()),
            ..Default::default()
        };
        assert_eq!(src.chat_type.0, "channel");
        let mt = MsgType("poll".to_string());
        assert!(!mt.is_media());
        assert_eq!(mt.to_string(), "poll");
    }

    #[test]
    fn json_roundtrip_of_raw_keeps_platform_fields() {
        let raw = json!({"msg_type": "post", "parent_id": "om_1"});
        let msg = InboundMessage {
            raw: raw.clone(),
            ..Default::default()
        };
        assert_eq!(msg.raw["parent_id"], json!("om_1"));
        assert_eq!(msg.raw, raw);
    }
}
