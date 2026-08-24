//! APIClient surface — port of `server/internal/integrations/lark/client.go`.
//!
//! The narrow async trait this crate needs from the Lark Open Platform HTTP
//! API. It is intentionally defined here (rather than taken from a vendor
//! SDK) so the rest of the crate can be built and unit-tested without
//! dragging Lark's transport into every test, and so implementations can be
//! swapped (real HTTP client, stub, fake) without touching call sites.
//!
//! All methods are scoped to a single installation — the caller has already
//! authenticated the installation row and decrypted its app_secret. The
//! client never reads the installation table itself.

use async_trait::async_trait;

use crate::types::{ChatId, OpenId, Region};

/// ListMessagesParams selects a bounded, recent window of messages in a
/// single Lark chat for the group-context prefetch. Only the fields the
/// enricher needs today are exposed (chat_id, thread_id, page_size,
/// end_time); start_time and page_token are intentionally omitted until a
/// caller needs them.
#[derive(Debug, Clone, Default)]
pub struct ListMessagesParams {
    pub chat_id: ChatId,
    /// When non-empty, scopes the list to a single Lark topic (话题): the
    /// client sends container_id_type=thread with the thread id as
    /// container_id instead of the chat container. This keeps a @-mention
    /// inside a topic from ever seeing sibling topics' messages (#5835).
    /// Lark's thread container does NOT accept end_time, so end_time is
    /// ignored on this path — the caller anchors the window client-side.
    /// Empty keeps the chat-level container.
    pub thread_id: String,
    /// How many of the most-recent messages to fetch. The client clamps it
    /// into Lark's valid 1..50 range.
    pub page_size: i32,
    /// When > 0, caps the window to messages created at or before this Unix
    /// timestamp in SECONDS (Lark's end_time is second-, not millisecond-,
    /// granularity). The enricher sets it to the trigger message's time so
    /// the prefetch is anchored to the @-mention moment rather than whatever
    /// is newest by the time the fetch runs. Ignored when thread_id is set.
    pub end_time: i64,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadResourceParams {
    pub message_id: String,
    pub file_key: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadedResource {
    pub data: Vec<u8>,
    pub content_type: String,
    pub filename: String,
    pub size_bytes: i64,
}

/// A streaming resource download. `body` is consumed once by the reader.
pub struct DownloadedResourceStream {
    pub body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    pub content_type: String,
    pub filename: String,
    pub size_bytes: i64,
}

impl std::fmt::Debug for DownloadedResourceStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadedResourceStream")
            .field("content_type", &self.content_type)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .finish_non_exhaustive()
    }
}

impl DownloadedResourceStream {
    /// Reads the whole body into memory (bounded by the transport's own
    /// 100 MiB guard).
    pub async fn read_to_end(mut self) -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut self.body, &mut buf).await?;
        Ok(buf)
    }
}

/// LarkMessage is the normalized slice of an IM v1 message item the enricher
/// needs. body.content is passed through raw (still the JSON-encoded,
/// msg_type-specific string Lark double-encodes) so the flattener — not the
/// transport client — owns content interpretation.
#[derive(Debug, Clone, Default)]
pub struct LarkMessage {
    pub message_id: String,
    /// Lark `msg_type`: text / post / image / merge_forward / …
    pub message_type: String,
    /// Raw body.content (a JSON-encoded string).
    pub content: String,
    /// sender.id (open_id for users, app_id for apps).
    pub sender_id: String,
    /// sender.sender_type: user / app / anonymous / …
    pub sender_type: String,
    /// Epoch milliseconds, as Lark returns it (a string).
    pub create_time: String,
    pub parent_id: String,
    pub root_id: String,
    /// Lark topic (话题) id; empty for messages outside a thread.
    pub thread_id: String,
    /// The merge_forward parent a child hangs under.
    pub upper_message_id: String,
    pub deleted: bool,
    pub mentions: Vec<LarkMessageMention>,
}

/// Mirrors a mentions[] entry on the IM REST item shape. Note this differs
/// from the WS receive event's mention shape: here `id` is a bare open_id
/// string, not a nested {open_id, union_id, user_id} object.
#[derive(Debug, Clone, Default)]
pub struct LarkMessageMention {
    /// e.g. "@_user_1"
    pub key: String,
    /// open_id
    pub id: String,
    /// Display name (may be empty).
    pub name: String,
}

/// BotInfo is the slice of /open-apis/bot/v3/info (+ a follow-up
/// /open-apis/contact/v3/users lookup for the union_id) we care about: the
/// Bot's per-installation `open_id` and its stable `union_id`.
///
/// Both identifiers are persisted on the installation config:
///
/// - `open_id` is the per-app Lark identifier; it is what /bot/v3/info
///   returns and what the OUTBOUND send paths use to address a user.
/// - `union_id` is the cross-app stable identifier scoped to the Lark
///   tenant. It is the only field that is consistent across the two WS
///   perspectives in a multi-bot group chat — see MUL-2671 group-@-mention
///   triage. The decoder matches inbound `mentions[].id` against `union_id`
///   so the right bot's supervisor handles the event when several bots are
///   bound to the same group.
///
/// Everything else /bot/v3/info returns (display name, avatar,
/// activate_status, ip_white_list) is intentionally dropped — those can be
/// re-fetched downstream from the bot_open_id if a UI needs them, and
/// freezing them in our schema would create a drift surface every time the
/// operator edits the Bot on Lark's side.
#[derive(Debug, Clone, Default)]
pub struct BotInfo {
    pub open_id: OpenId,
    pub union_id: String,
}

/// SendCardParams is the input shape for posting a fresh card.
#[derive(Debug, Clone)]
pub struct SendCardParams {
    pub installation_id: InstallationCredentials,
    pub chat_id: ChatId,
    /// The raw Lark interactive card JSON body. Passed through opaque so the
    /// card-template package can evolve without dragging this transport
    /// interface along.
    pub card_json: String,
    /// When set, routes the send through Lark's reply endpoint
    /// (POST /im/v1/messages/{id}/reply) instead of the chat-level send
    /// endpoint, so the card lands inside the originating 话题 (thread).
    /// Empty reply_target keeps the legacy chat-level send.
    pub reply_target: ReplyTarget,
}

/// ReplyTarget describes how an outbound message should be threaded back to
/// an inbound message. When message_id is non-empty the transport uses Lark's
/// reply endpoint targeting that message; in_thread maps to the
/// reply_in_thread flag so the reply stays inside the message's topic. The
/// zero value (empty message_id) means "send at the chat level" — the
/// historical behavior — so callers that don't care about threading just
/// leave it unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplyTarget {
    pub message_id: String,
    pub in_thread: bool,
}

impl ReplyTarget {
    /// Reports whether this target should route through the reply endpoint.
    /// A reply needs a parent message_id; without one there is nothing to
    /// reply to and the caller falls back to a chat-level send.
    pub fn is_set(&self) -> bool {
        !self.message_id.is_empty()
    }
}

/// PatchCardParams is the input shape for updating an existing card.
#[derive(Debug, Clone)]
pub struct PatchCardParams {
    pub installation_id: InstallationCredentials,
    pub lark_card_message_id: String,
    pub card_json: String,
}

/// SendTextParams is the input shape for posting a plain text message. Text
/// is sent verbatim to Lark; the client handles JSON encoding of the
/// `{"text": "..."}` content envelope Lark requires.
#[derive(Debug, Clone)]
pub struct SendTextParams {
    pub installation_id: InstallationCredentials,
    pub chat_id: ChatId,
    pub text: String,
    /// Threads the text reply back into a Lark topic; see [`ReplyTarget`].
    /// Empty keeps the chat-level send.
    pub reply_target: ReplyTarget,
}

/// SendMarkdownCardParams is the input shape for posting an agent reply as a
/// Lark interactive card with a markdown body element. Markdown is forwarded
/// to Lark verbatim; the client builds the schema-2.0 card envelope around it.
#[derive(Debug, Clone)]
pub struct SendMarkdownCardParams {
    pub installation_id: InstallationCredentials,
    pub chat_id: ChatId,
    /// The body. Lark schema-2.0 markdown supports GFM-ish: **bold**,
    /// *italic*, `inline code`, fenced code blocks, headings, ordered +
    /// unordered lists, links, tables, blockquotes, separators.
    pub markdown: String,
    /// When non-empty, rendered as the single-line preview Lark shows in the
    /// chat list / desktop notification. Empty falls back to whatever Lark
    /// derives from the body.
    pub summary: String,
    /// Threads the card reply back into a Lark topic; see [`ReplyTarget`].
    pub reply_target: ReplyTarget,
}

/// BindingPromptParams carries the data needed to render and send the
/// member-binding prompt card (single CTA: open the binding URL).
#[derive(Debug, Clone)]
pub struct BindingPromptParams {
    pub installation_id: InstallationCredentials,
    pub open_id: OpenId,
    /// The absolute URL the user clicks. The token is embedded in the URL by
    /// the caller; the client never sees it.
    pub bind_url: String,
}

/// AddReactionParams is the input shape for adding an emoji reaction to a
/// message.
#[derive(Debug, Clone)]
pub struct AddReactionParams {
    pub installation_id: InstallationCredentials,
    pub message_id: String,
    pub emoji_type: String,
}

/// DeleteReactionParams is the input shape for removing a previously-added
/// reaction from a message.
#[derive(Debug, Clone)]
pub struct DeleteReactionParams {
    pub installation_id: InstallationCredentials,
    pub message_id: String,
    pub reaction_id: String,
}

/// InstallationCredentials is the per-installation transport context the
/// client needs to authenticate against Lark on behalf of a workspace's bot.
/// Passing these explicitly to each call (rather than constructing
/// per-installation clients) keeps lifecycle simple: the hub decrypts
/// app_secret once and reuses the struct for every outbound call.
///
/// The plaintext app_secret lives inside this struct exactly while a call is
/// in flight; callers MUST NOT log or persist it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallationCredentials {
    pub app_id: String,
    pub app_secret: String,
    pub tenant_key: String,
    /// Selects the Lark open-platform host (Feishu mainland vs Lark
    /// international) for every call made with these credentials. Empty
    /// defaults to Feishu. Credential-build sites copy it from the
    /// installation config; the device-flow installer sets it from the
    /// auto-detected tenant. This is what lets one deployment serve both
    /// clouds — see http_client resolve_base_url and ws_endpoint.
    pub region: Region,
}

/// Sentinel error returned by the stub client to signal that a real Lark
/// client has not been wired in yet. Call sites SHOULD treat this as an
/// expected condition on self-host deployments without a Lark app — log a
/// warning, fall back to "Lark integration not configured", and continue
/// serving other workspace functionality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark: API client not configured")]
pub struct ApiClientNotConfigured;

/// APIClient is the narrow async surface this crate needs from the Lark Open
/// Platform HTTP API. See the module docs for the boundary rationale.
#[async_trait]
pub trait ApiClient: Send + Sync {
    /// Reports whether this ApiClient can reach Lark over the network. It is
    /// the "HTTP outbound is wired" signal: the stub returns false; the real
    /// Lark HTTP client returns true once instantiated. Handlers consult this
    /// when deciding whether to surface install / management UI that needs to
    /// talk to Lark.
    fn is_configured(&self) -> bool;

    /// Posts an interactive card into a Lark chat and returns Lark's
    /// message_id for the card. The patcher persists this id in
    /// channel_outbound_card_message so subsequent patches can target the
    /// same card.
    async fn send_interactive_card(&self, p: SendCardParams) -> anyhow::Result<String>;

    /// Replaces the body of a previously-sent card. The throttling decision
    /// belongs to the caller; this method just performs the network call.
    async fn patch_interactive_card(&self, p: PatchCardParams) -> anyhow::Result<()>;

    /// Posts a plain text message into a Lark chat. Used for the agent's chat
    /// reply when the body has no markdown syntax — short prose /
    /// acknowledgments / pings. A plain text bubble feels like a normal IM
    /// message; we deliberately keep this path even after adding the markdown
    /// card variant because wrapping a one-liner "Hello!" inside a card just
    /// adds visual chrome the user doesn't want.
    async fn send_text_message(&self, p: SendTextParams) -> anyhow::Result<String>;

    /// Posts the agent's reply as a Lark interactive card (schema 2.0) with a
    /// single `tag: "markdown"` body element. This is the path the chat-reply
    /// router takes when the body contains markdown syntax (fenced code
    /// blocks, headings, lists, tables, etc.) — Lark renders the markdown
    /// into formatted text rather than leaving raw `**bold**` / `# heading`
    /// characters in the user's transcript. Returns the card's message_id.
    async fn send_markdown_card(&self, p: SendMarkdownCardParams) -> anyhow::Result<String>;

    /// The dedicated "you need to bind" outbound. Kept separate from
    /// send_interactive_card so the abstraction stays stable when the
    /// production card template changes — call sites in identity check don't
    /// have to know about Lark's card schema.
    async fn send_binding_prompt_card(&self, p: BindingPromptParams) -> anyhow::Result<()>;

    /// Returns the Bot's per-installation `open_id` (the `bot_open_id` we
    /// persist on the installation config). RegistrationService is the only
    /// caller — after the device-flow registration returns fresh
    /// `client_id` / `client_secret`, the service mints a tenant_access_token
    /// with those creds and calls /open-apis/bot/v3/info to learn the Bot's
    /// identity. The result is then frozen into the installation alongside
    /// the app_id / app_secret in the same transaction as the installer-bind.
    async fn get_bot_info(&self, creds: InstallationCredentials) -> anyhow::Result<BotInfo>;

    /// Fetches a message by id via GET /open-apis/im/v1/messages/{message_id}.
    /// Lark always returns an ARRAY (data.items[]): for a normal message
    /// exactly one element; for a `merge_forward` message the first element
    /// is the forward sentinel and the remaining elements are the bundled
    /// child messages (each a normal typed message linked back by
    /// upper_message_id). The inbound enricher relies on both shapes:
    /// items[0] for a quoted-reply parent, items[1:] for a forwarded
    /// transcript. Returning the raw vec keeps this method a thin transport
    /// adapter — flattening and block assembly are the enricher's job.
    async fn get_message(
        &self,
        creds: InstallationCredentials,
        message_id: &str,
    ) -> anyhow::Result<Vec<LarkMessage>>;

    /// Fetches the most recent messages in a single chat via
    /// GET /open-apis/im/v1/messages. It powers the group-context prefetch:
    /// when a user @-mentions the Bot in a busy group, the enricher pulls a
    /// bounded window of surrounding messages so the agent sees the
    /// conversation, not just the one @-ed line.
    ///
    /// Results come back newest-first (sort_type=ByCreateTimeDesc), capped at
    /// p.page_size (Lark hard-caps a page at 50); the caller orders and trims
    /// for rendering. Only a single page is fetched — pagination is
    /// deliberately not exposed so the inbound ACK path's HTTP fan-out stays
    /// a single round-trip. Like get_message, this is a thin transport
    /// adapter: flattening and block assembly are the enricher's job.
    async fn list_chat_messages(
        &self,
        creds: InstallationCredentials,
        p: ListMessagesParams,
    ) -> anyhow::Result<Vec<LarkMessage>>;

    /// Downloads one binary resource attached to a message via
    /// GET /open-apis/im/v1/messages/{message_id}/resources/{file_key}.
    /// Type is the Open Platform resource class ("image" for image_key,
    /// "file" for file_key-backed video/file/audio).
    async fn download_message_resource(
        &self,
        creds: InstallationCredentials,
        p: DownloadResourceParams,
    ) -> anyhow::Result<DownloadedResource>;

    async fn download_message_resource_stream(
        &self,
        creds: InstallationCredentials,
        p: DownloadResourceParams,
    ) -> anyhow::Result<DownloadedResourceStream> {
        let resource = self.download_message_resource(creds, p).await?;
        Ok(DownloadedResourceStream {
            body: Box::new(std::io::Cursor::new(resource.data)),
            content_type: resource.content_type,
            filename: resource.filename,
            size_bytes: resource.size_bytes,
        })
    }

    /// Resolves a set of user open_ids to their display names via
    /// GET /open-apis/contact/v3/users/batch. The enricher uses it to label
    /// recent-context / quoted / forwarded speakers (and the sender who
    /// @-mentioned the Bot) with real names instead of positional "User 1 /
    /// User 2". Returns an open_id → name map; ids the API does not return
    /// (restricted contact scope, deactivated user, …) are simply absent from
    /// the map, and the caller falls back to a positional label. open_ids
    /// beyond Lark's 50-per-call cap are dropped by the client.
    async fn batch_get_users(
        &self,
        creds: InstallationCredentials,
        open_ids: &[String],
    ) -> anyhow::Result<std::collections::HashMap<String, String>>;

    /// Adds an emoji reaction to an existing message. The standard use-case
    /// is the "Typing" indicator that signals the Bot is processing the
    /// user's message. Returns the reaction_id Lark assigns so it can be
    /// removed later.
    async fn add_message_reaction(&self, p: AddReactionParams) -> anyhow::Result<String>;

    /// Removes a previously-added reaction from a message. This is the
    /// cleanup half of the typing-indicator lifecycle.
    async fn delete_message_reaction(&self, p: DeleteReactionParams) -> anyhow::Result<()>;
}

/// stub_api_client is the default ApiClient used when no production client
/// has been registered. It refuses every transport call with
/// [`ApiClientNotConfigured`] so a misconfigured deployment fails loudly
/// instead of silently dropping cards or device-flow registration responses.
///
/// We deliberately do NOT silently succeed: a stub that returned "" message
/// ids would let the inbound dispatcher record bogus outbound-card rows
/// pointing at nothing.
pub struct StubApiClient;

impl StubApiClient {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StubApiClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApiClient for StubApiClient {
    fn is_configured(&self) -> bool {
        false
    }

    async fn send_interactive_card(&self, p: SendCardParams) -> anyhow::Result<String> {
        tracing::warn!(
            chat_id = %p.chat_id,
            "lark stub client: send_interactive_card called"
        );
        Err(ApiClientNotConfigured.into())
    }

    async fn patch_interactive_card(&self, p: PatchCardParams) -> anyhow::Result<()> {
        tracing::warn!(
            card_message_id = %p.lark_card_message_id,
            "lark stub client: patch_interactive_card called"
        );
        Err(ApiClientNotConfigured.into())
    }

    async fn send_text_message(&self, p: SendTextParams) -> anyhow::Result<String> {
        tracing::warn!(chat_id = %p.chat_id, "lark stub client: send_text_message called");
        Err(ApiClientNotConfigured.into())
    }

    async fn send_markdown_card(&self, p: SendMarkdownCardParams) -> anyhow::Result<String> {
        tracing::warn!(chat_id = %p.chat_id, "lark stub client: send_markdown_card called");
        Err(ApiClientNotConfigured.into())
    }

    async fn send_binding_prompt_card(&self, p: BindingPromptParams) -> anyhow::Result<()> {
        tracing::warn!(open_id = %p.open_id, "lark stub client: send_binding_prompt_card called");
        Err(ApiClientNotConfigured.into())
    }

    async fn get_bot_info(&self, creds: InstallationCredentials) -> anyhow::Result<BotInfo> {
        tracing::warn!(app_id = %creds.app_id, "lark stub client: get_bot_info called");
        Err(ApiClientNotConfigured.into())
    }

    async fn get_message(
        &self,
        _creds: InstallationCredentials,
        message_id: &str,
    ) -> anyhow::Result<Vec<LarkMessage>> {
        tracing::warn!(message_id, "lark stub client: get_message called");
        Err(ApiClientNotConfigured.into())
    }

    async fn list_chat_messages(
        &self,
        _creds: InstallationCredentials,
        p: ListMessagesParams,
    ) -> anyhow::Result<Vec<LarkMessage>> {
        tracing::warn!(chat_id = %p.chat_id, "lark stub client: list_chat_messages called");
        Err(ApiClientNotConfigured.into())
    }

    async fn download_message_resource(
        &self,
        _creds: InstallationCredentials,
        p: DownloadResourceParams,
    ) -> anyhow::Result<DownloadedResource> {
        tracing::warn!(message_id = %p.message_id, "lark stub client: download_message_resource called");
        Err(ApiClientNotConfigured.into())
    }

    async fn batch_get_users(
        &self,
        _creds: InstallationCredentials,
        open_ids: &[String],
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        tracing::warn!(
            count = open_ids.len(),
            "lark stub client: batch_get_users called"
        );
        Err(ApiClientNotConfigured.into())
    }

    async fn add_message_reaction(&self, p: AddReactionParams) -> anyhow::Result<String> {
        tracing::warn!(
            message_id = %p.message_id,
            emoji_type = %p.emoji_type,
            "lark stub client: add_message_reaction called"
        );
        Err(ApiClientNotConfigured.into())
    }

    async fn delete_message_reaction(&self, p: DeleteReactionParams) -> anyhow::Result<()> {
        tracing::warn!(
            message_id = %p.message_id,
            reaction_id = %p.reaction_id,
            "lark stub client: delete_message_reaction called"
        );
        Err(ApiClientNotConfigured.into())
    }
}
