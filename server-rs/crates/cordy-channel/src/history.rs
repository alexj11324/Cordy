//! Channel-agnostic vocabulary for ON-DEMAND history reads.
//!
//! Shared channel-history contract (PB-3871).
//! History is PULLED by the agent through two unified CLI commands —
//! `cordy chat history` (the channel OVERVIEW: top-level messages +
//! thread metadata, not thread contents) and `cordy chat thread [id]`
//! (one thread's messages). The agent never sees a per-platform API: the
//! server resolves the session's binding to a channel type and dispatches
//! to that platform's reader, which returns these normalized shapes.
//! Adding a platform is "implement a reader"; the agent-facing contract
//! never changes.

use serde::{Deserialize, Serialize};

/// The normalized author kind of a fetched message, mirroring the
/// chat_message.role domain the agent already reasons about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRole(pub String);

impl HistoryRole {
    /// A human (or a third-party bot, e.g. an alerting bot) message —
    /// context the agent should read.
    pub fn user() -> HistoryRole {
        HistoryRole("user".to_string())
    }
    /// One of THIS bot's own prior messages in the conversation.
    pub fn assistant() -> HistoryRole {
        HistoryRole("assistant".to_string())
    }
}

impl Default for HistoryRole {
    fn default() -> Self {
        Self::user()
    }
}

/// One normalized message. It is the same shape regardless of platform so
/// the agent reads a uniform list, like `cordy issue comment list --output
/// json`. Serde field names mirror the Go json tags byte-for-byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// The platform message identifier (Slack ts, Feishu message_id).
    pub id: String,
    /// A human-readable display label for the sender ("Alice", "Bot", or
    /// a positional "User 2" fallback when the name is unresolved).
    pub author: String,
    /// The platform-native sender id, when available. Empty for some
    /// platform/bot messages.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author_id: String,
    /// Distinguishes the bot's own turns from everyone else's.
    pub role: HistoryRole,
    /// The message body, flattened to plain text by the adapter.
    pub text: String,
    /// The platform timestamp string, sortable lexicographically within a
    /// platform (Slack "1700000000.000100"). It doubles as the paging
    /// cursor.
    pub ts: String,

    // The following are set only on a CHANNEL-OVERVIEW row that heads a
    // thread, so the agent can `cordy chat thread <thread_id>` to read its
    // contents. They are absent on a plain message and on thread-read rows.
    /// The identifier to pass to `cordy chat thread <id>` to read this
    /// thread's messages. Set only when this overview row has a thread.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    /// How many replies the thread has (omitted when none).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reply_count: i64,
    /// The platform timestamp of the most recent reply, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_reply: String,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// One normalized page. Messages are ordered OLDEST-FIRST so the
/// transcript reads top-to-bottom like the chat does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPage {
    /// The platform the history came from ("slack"). Empty when the
    /// session is not bound to any channel (a web-only chat session).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_type: String,
    /// Set on a THREAD read: which thread these messages belong to.
    /// Empty on a channel overview.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    /// The fetched messages, oldest-first.
    pub messages: Vec<HistoryMessage>,
    /// When non-empty, an opaque cursor to pass as `before` to page to
    /// OLDER messages. Empty means no older messages were available.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_cursor: String,
}

/// Tune a history read. They are platform-neutral; each reader maps them
/// onto its own API's paging primitives.
#[derive(Debug, Clone, Default)]
pub struct HistoryOptions {
    /// Caps how many messages to return. A reader clamps it to its
    /// platform's per-page maximum and applies a sane default for <= 0.
    pub limit: i64,
    /// An opaque cursor (a `next_cursor` from a prior page); the reader
    /// returns only messages strictly older than it. Empty starts at the
    /// most recent messages.
    pub before: String,
}
