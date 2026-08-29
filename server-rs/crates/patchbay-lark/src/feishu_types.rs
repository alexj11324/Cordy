//! The Feishu adapter's native-ish inbound/outbound value types.
//!
//! The WS connector decodes a raw Lark event into an [`InboundMessage`];
//! the channel adapter translates that into a
//! `patchbay_channel::InboundMessage` for the channel-agnostic engine Router,
//! and the resolvers / OutcomeReplier translate back at the adapter boundary.

use uuid::Uuid;

use crate::types::{ChatId, ChatType, DropReason, OpenId};

/// patchbay_channel::ChatType is a plain newtype without serde derives; the raw
/// envelope carries it as its string value.
mod chat_type_serde {
    use patchbay_channel::message::ChatType;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(t: &ChatType, s: S) -> Result<S::Ok, S::Error> {
        t.0.serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ChatType, D::Error> {
        Ok(ChatType(String::deserialize(d)?))
    }
}

/// InboundMessage is the Feishu connector's decoded, enriched event. It is
/// the adapter's internal shape: the channel adapter maps it to a
/// patchbay_channel::InboundMessage (stashing this struct in Raw) so the
/// resolvers can read the platform-specific fields the normalized envelope
/// does not carry.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct InboundMessage {
    pub event_type: String,
    pub event_id: String,
    pub app_id: String,
    pub chat_id: ChatId,
    #[serde(with = "chat_type_serde")]
    pub chat_type: ChatType,
    pub message_id: String,
    pub sender_open_id: OpenId,
    /// A direct control-plane response such as `/agents` output.
    pub reply_text: Option<String>,
    pub body: String,
    /// Content is the raw msg_type-specific JSON string Lark sends in
    /// event.message.content. Text/post decoding consumes it immediately;
    /// media ingestion keeps it so the adapter can extract image_key /
    /// file_key before translating to the normalized envelope.
    pub content: String,
    /// ForceFreshSession marks this dispatch as a one-off fresh start: the
    /// daemon should skip prior session resume when it claims the resulting
    /// chat task.
    pub force_fresh_session: bool,
    pub addressed_to_bot: bool,

    /// MessageType is the raw Lark msg_type ("text", "post", "merge_forward",
    /// "image", "interactive", …). The decoder populates it so the inbound
    /// enricher can decide whether a message needs an HTTP round-trip to
    /// expand while the core stays msg_type-agnostic and only reads body.
    pub message_type: String,

    /// CreateTime is the trigger message's creation time (epoch milliseconds).
    /// The enricher anchors the group recent-context window to it; the typing
    /// indicator uses it to skip stale reactions.
    pub create_time: String,

    /// ParentID is the message_id this one quote-replies to (verbatim
    /// parent_id); RootID is the thread/root anchor. The enricher expands
    /// quoted replies off parent_id.
    pub parent_id: String,
    pub root_id: String,

    /// ThreadID is the Lark topic (话题) id, populated only for messages
    /// posted inside a thread, so a non-empty value signals an in-thread
    /// @-mention. Persisted on the chat binding so the outbound patcher
    /// threads its reply.
    pub thread_id: String,

    /// CommandBody is the user's OWN typed text (the decoded body before the
    /// enricher prepends quoted/forwarded context). `/issue` is parsed from
    /// THIS, not the enriched body.
    pub command_body: String,
}

/// DispatchResult is the Feishu-side verdict the OutcomeReplier consumes to
/// drive its outbound reply. The engine produces an engine Result; the
/// OutboundReplier adapter translates it into this shape.
///
/// Port note: Go's pgtype.UUID fields become Option<Uuid>; Outcome and
/// DropReason reuse the engine's open-string newtypes (values match 1:1).
#[derive(Debug, Clone, Default)]
pub struct DispatchResult {
    pub outcome: Option<patchbay_channel_engine::resolvers::Outcome>,
    pub drop_reason: Option<DropReason>,
    pub installation_id: Option<Uuid>,
    pub chat_session_id: Option<Uuid>,
    pub sender_open_id: OpenId,
    /// Direct control-plane text such as the `/agents` list or switch reply.
    pub reply_text: Option<String>,
    pub task_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub issue_number: i32,
    /// IssueIdentifier is the workspace-qualified key ("PB-42") for the
    /// created issue, used verbatim in the confirmation message.
    pub issue_identifier: String,
    /// IssueTitle is the title supplied on /issue, echoed in the confirmation.
    pub issue_title: String,
    /// IssueDuplicate distinguishes an active-issue conflict from a
    /// successful create while carrying the existing issue fields above.
    pub issue_duplicate: bool,
    /// IssueUsageHadMedia asks the usage reply to tell the sender to include
    /// the current message's media again with the corrected command.
    pub issue_usage_had_media: bool,
}

impl DispatchResult {
    /// The outcome label, or "" when unset (Go's zero value).
    pub fn outcome_str(&self) -> &str {
        self.outcome.as_ref().map_or("", |o| &o.0)
    }

    pub fn outcome_is(&self, want: &str) -> bool {
        self.outcome.as_ref().is_some_and(|o| o.0 == want)
    }
}

/// Outcome labels mirror engine.Outcome; re-exported constructors keep call
/// sites readable without importing the engine module everywhere.
pub mod outcome {
    pub use patchbay_channel_engine::resolvers::Outcome;

    /// Not ingested (identity, dedup, group filter, …).
    pub fn dropped() -> Outcome {
        Outcome::dropped()
    }
    /// The open_id is unbound; send the binding card.
    pub fn needs_binding() -> Outcome {
        Outcome::needs_binding()
    }
    /// The message landed and a run was (or will be) enqueued.
    pub fn ingested() -> Outcome {
        Outcome::ingested()
    }
    /// A bare /new was persisted for the next chat turn.
    pub fn fresh_pending() -> Outcome {
        Outcome::fresh_pending()
    }
    /// /issue was sent without its required title.
    pub fn issue_usage() -> Outcome {
        Outcome::issue_usage()
    }
    /// Landed, but the agent has no runtime bound.
    pub fn agent_offline() -> Outcome {
        Outcome::agent_offline()
    }
    /// Landed, but the agent is archived.
    pub fn agent_archived() -> Outcome {
        Outcome::agent_archived()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_helpers_match_engine_values() {
        assert_eq!(outcome::dropped().0, "dropped");
        assert_eq!(outcome::needs_binding().0, "needs_binding");
        assert_eq!(outcome::ingested().0, "ingested");
        assert_eq!(outcome::fresh_pending().0, "fresh_pending");
        assert_eq!(outcome::issue_usage().0, "issue_usage");
        assert_eq!(outcome::agent_offline().0, "agent_offline");
        assert_eq!(outcome::agent_archived().0, "agent_archived");

        let res = DispatchResult {
            outcome: Some(outcome::needs_binding()),
            ..Default::default()
        };
        assert!(res.outcome_is("needs_binding"));
        assert!(!res.outcome_is("dropped"));
        assert_eq!(res.outcome_str(), "needs_binding");
        assert_eq!(DispatchResult::default().outcome_str(), "");
    }

    #[test]
    fn default_inbound_message_has_empty_chat_type() {
        let m = InboundMessage::default();
        assert_eq!(m.chat_type.0, "");
        assert!(!m.force_fresh_session);
        assert!(!m.addressed_to_bot);
    }
}
