//! Port of execenv/channel_type.go.
//!
//! Symbol map:
//! - ChannelTypeSlack/Feishu/Wecom/Dingtalk → CHANNEL_TYPE_* consts
//! - SurfacePersistsTranscript              → surface_persists_transcript
//! - ChatTypeP2P / ChatTypeGroup            → CHAT_TYPE_P2P / CHAT_TYPE_GROUP
//! - ChatAudience (+Unknown/Direct/Group)   → ChatAudience enum
//! - AudienceOf                             → audience_of
//! - ChannelCarriesFiles                    → channel_carries_files
//! - ChannelDisplayName                     → channel_display_name

/// Chat channel discriminators as they arrive on the task payload. The server
/// stamps `chat_channel_type` from the channel_chat_session_binding row
/// (handler/daemon.go); an empty value means a web/mobile chat session with no
/// IM channel behind it.
///
/// These are plain string constants on purpose: the daemon compares a value the
/// server already serialized to JSON, and must not pull the server-side
/// integration packages into its own build just to read one discriminator. The
/// canonical definitions live with their adapters and both sides agree on the
/// wire strings below.
pub const CHANNEL_TYPE_SLACK: &str = "slack";
pub const CHANNEL_TYPE_FEISHU: &str = "feishu";
pub const CHANNEL_TYPE_WECOM: &str = "wecom";
pub const CHANNEL_TYPE_DINGTALK: &str = "dingtalk";

/// Room-shape discriminators, mirroring channel_chat_session_binding.chat_type
/// (channel.ChatTypeP2P / channel.ChatTypeGroup). Every adapter persists this
/// column, so the shape of a conversation is known off one read whatever the
/// platform. Empty means the server did not report one — a web chat, which has
/// no binding row, or a server predating the field.
pub const CHAT_TYPE_P2P: &str = "p2p";
pub const CHAT_TYPE_GROUP: &str = "group";

/// ChatAudience is what a run is allowed to say about who can read its replies.
/// Three states, because "unknown" is not "private": a web chat carries no
/// binding row at all and is 1:1 by construction, but an IM channel whose shape
/// the server did not report could be a room of any size, and the one thing the
/// copy must not then do is promise a privacy the conversation may not have.
///
/// The per-turn chat prompt names the audience once. Keeping classification in
/// one function prevents group, direct, and compatibility paths from drifting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAudience {
    /// Deliberately the default value: uninitialized room context must never
    /// turn into a privacy claim.
    Unknown,
    /// An explicit p2p binding, or the no-channel shape used by web chat. The
    /// claim protocol cannot distinguish a deleted binding here.
    Direct,
    /// A room shared by people the run has not been shown.
    Group,
}

/// SurfacePersistsTranscript reports whether a chat surface stores its
/// conversation in Patchbay's chat_message table, readable back via `patchbay chat
/// history` (handler/chat_history.go's non-Slack fallback). Web chat (empty
/// discriminator), Feishu, WeCom and DingTalk all persist via the shared
/// AppendUserMessage path; Slack reads the live channel instead. It is the single
/// source of truth for "which surfaces are readable", shared by the
/// continuity-notice router, the chat-prompt history copy, and the surface list —
/// so a future non-persisting channel is never told "read it back".
pub fn surface_persists_transcript(channel_type: &str) -> bool {
    matches!(
        channel_type,
        "" | CHANNEL_TYPE_FEISHU | CHANNEL_TYPE_WECOM | CHANNEL_TYPE_DINGTALK
    )
}

/// AudienceOf classifies a claim's (chat_channel_type, chat_type) pair.
pub fn audience_of(channel_type: &str, chat_type: &str) -> ChatAudience {
    if chat_type == CHAT_TYPE_GROUP {
        return ChatAudience::Group;
    }
    if chat_type == CHAT_TYPE_P2P {
        return ChatAudience::Direct;
    }
    if channel_type.is_empty() {
        return ChatAudience::Direct;
    }
    ChatAudience::Unknown
}

/// ChannelCarriesFiles reports whether a file the agent produces will actually
/// reach this conversation. It is the delivery half of the two-layer channel
/// policy (PB-4899).
///
/// Its one caller is the per-turn chat prompt (daemon.buildChatPrompt). The
/// runtime brief must not call it: the answer changes turn to turn on one
/// resumed session, and the brief is the prompt-cache prefix (PB-5377).
///
/// `server_says_delivers` is the claim's chat_channel_delivers_files, and it is
/// the ONLY thing consulted for a channel-backed chat. The channel type is not,
/// and the temptation to answer from it is the defect this signature exists to
/// prevent: whether the last hop happens takes an adapter that goes back for the
/// bound attachment AND object storage for it to go back to, and the second half
/// is a deployment fact no daemon can observe.
///
/// Web / mobile chat is not answered here. It has no channel type at all and is
/// handled by its own branch, which points at the attachment card the browser
/// renders rather than at an IM message.
pub fn channel_carries_files(channel_type: &str, server_says_delivers: bool) -> bool {
    if channel_type.is_empty() {
        return false;
    }
    server_says_delivers
}

/// ChannelDisplayName renders a chat_channel_type for prompt / brief copy.
/// Unknown types fall through to the raw discriminator rather than a generic
/// placeholder, so a channel added server-side without a mapping here still
/// names itself in the prompt instead of silently reading as "unknown".
pub fn channel_display_name(channel_type: &str) -> String {
    match channel_type {
        CHANNEL_TYPE_SLACK => "Slack".to_string(),
        CHANNEL_TYPE_FEISHU => "Feishu/Lark".to_string(),
        CHANNEL_TYPE_WECOM => "WeCom".to_string(),
        CHANNEL_TYPE_DINGTALK => "DingTalk".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestChannelCarriesFiles.
    #[test]
    fn test_channel_carries_files() {
        // Web / mobile chat never answers here even when the flag arrives set.
        assert!(!channel_carries_files("", true));
        // Channel-backed chats answer exactly as the server said.
        assert!(channel_carries_files(CHANNEL_TYPE_SLACK, true));
        assert!(!channel_carries_files(CHANNEL_TYPE_SLACK, false));
        assert!(channel_carries_files(CHANNEL_TYPE_WECOM, true));
        assert!(!channel_carries_files(CHANNEL_TYPE_WECOM, false));
    }

    // Port of TestEveryKnownChannelHasADisplayName (known-channel half; the Go
    // test walks integration-package constants that do not exist in this crate).
    #[test]
    fn test_known_channels_have_display_names() {
        for ct in [
            CHANNEL_TYPE_SLACK,
            CHANNEL_TYPE_FEISHU,
            CHANNEL_TYPE_WECOM,
            CHANNEL_TYPE_DINGTALK,
        ] {
            let name = channel_display_name(ct);
            assert_ne!(name, "", "no display name for {ct}");
            assert_ne!(name, ct, "display name falls through for {ct}");
        }
        // Unknown types fall through to the raw discriminator.
        assert_eq!(channel_display_name("matrix"), "matrix");
    }

    #[test]
    fn test_audience_of() {
        assert_eq!(audience_of("", ""), ChatAudience::Direct);
        assert_eq!(audience_of("", CHAT_TYPE_P2P), ChatAudience::Direct);
        assert_eq!(audience_of("", CHAT_TYPE_GROUP), ChatAudience::Group);
        assert_eq!(audience_of(CHANNEL_TYPE_SLACK, ""), ChatAudience::Unknown);
        assert_eq!(
            audience_of(CHANNEL_TYPE_SLACK, CHAT_TYPE_P2P),
            ChatAudience::Direct
        );
        assert_eq!(
            audience_of(CHANNEL_TYPE_SLACK, CHAT_TYPE_GROUP),
            ChatAudience::Group
        );
    }

    #[test]
    fn test_surface_persists_transcript() {
        assert!(surface_persists_transcript(""));
        assert!(surface_persists_transcript(CHANNEL_TYPE_FEISHU));
        assert!(surface_persists_transcript(CHANNEL_TYPE_WECOM));
        assert!(surface_persists_transcript(CHANNEL_TYPE_DINGTALK));
        assert!(!surface_persists_transcript(CHANNEL_TYPE_SLACK));
    }
}
