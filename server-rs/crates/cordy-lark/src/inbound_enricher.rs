//! Inbound enricher — port of
//! `server/internal/integrations/lark/inbound_enricher.go`.
//!
//! Expands an inbound message's body with context the user EXPLICITLY
//! attached — a quoted reply or a merged-and-forwarded bundle — by calling
//! back into Lark's IM API. It runs after the (fast, HTTP-free) decoder and
//! before the dispatcher, turning a bare "@bot 总结一下" into a body that
//! already carries the referenced conversation inline.
//!
//! It is best-effort by contract: every fetch failure degrades to a readable
//! note or placeholder and enrich NEVER returns an error or blocks ingestion.
//! A message with nothing to expand (no parent_id, not a merge_forward) is
//! returned untouched without any network call.
//!
//! Port note: Go threads a ctx through Enrich so the connector's enrichment
//! budget cancels doomed retries mid-flight; here the connector bounds the
//! whole call with a tokio timeout instead, which drops any in-flight retry
//! at the same deadline without threading cancellation into every fetch.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::client::{
    ApiClient, InstallationCredentials, LarkMessage, LarkMessageMention, ListMessagesParams,
};
use crate::content_flatten::{flatten_content, LARK_MSG_TYPE_MERGE_FORWARD};
use crate::feishu_types::InboundMessage;
use crate::frame_decoder::{resolve_mentions, LarkMention, LarkMentionId};
use crate::http_client::{is_token_error, ApiError};
use crate::types::{chat_type_group, ChatId};

/// Caps how many child messages we inline from a single forward. Lark itself
/// bounds a merge_forward at 100 messages; we mirror that as a safety valve so
/// a pathological bundle can't blow up the agent's context. Anything beyond
/// the cap is dropped with a visible "... (N more truncated)" marker.
pub const DEFAULT_MAX_FORWARD_CHILDREN: usize = 100;

/// The window the production wiring uses for the group-context prefetch: the
/// page_size of the single list call made when a user @-mentions the Bot in a
/// group. It is a FETCH budget, not a guaranteed rendered count — the trigger
/// message itself and any quoted parent are filtered out of the result, so
/// the <recent_context> block usually renders one or two fewer lines. 10
/// keeps the agent's prompt meaningfully contextual without bloating it or
/// straining the inbound ACK budget (one list call, page_size 10).
pub const DEFAULT_RECENT_CONTEXT_SIZE: i32 = 10;

const RECENT_CONTEXT_ENDPOINT: &str = "im/v1/messages.list";
const RECENT_CONTEXT_MAX_FETCH_ATTEMPTS: usize = 2;

// Failure categories for the recent-context fetch classification.
pub const RECENT_CONTEXT_FAILURE_UNKNOWN: &str = "unknown";
pub const RECENT_CONTEXT_FAILURE_CHANNEL_UNBOUND: &str = "channel_unbound";
pub const RECENT_CONTEXT_FAILURE_TIMEOUT: &str = "timeout";
pub const RECENT_CONTEXT_FAILURE_PERMISSION_DENIED: &str = "permission_denied";
pub const RECENT_CONTEXT_FAILURE_MESSAGE_DELETED: &str = "message_deleted";
pub const RECENT_CONTEXT_FAILURE_RATE_LIMITED: &str = "rate_limited";
pub const RECENT_CONTEXT_FAILURE_TOKEN_EXPIRED: &str = "token_expired";
pub const RECENT_CONTEXT_FAILURE_TEMPORARY: &str = "temporary";

/// Sentinel error for a message whose chat binding is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark enricher: missing chat_id for recent context")]
pub struct ErrRecentContextChannelUnbound;

/// Expands inbound message bodies with explicitly-attached context.
#[async_trait]
pub trait Enricher: Send + Sync {
    /// Rewrites msg.body to inline surrounding group context and/or any
    /// quoted-reply parent and/or forwarded bundle. Composition order goes
    /// broadest-to-narrowest: the surrounding group history first, then the
    /// explicitly-quoted parent (a specific reference), then the message's
    /// own content (or, for a forward, the rendered transcript).
    ///
    /// ```text
    /// <recent_context …>…</recent_context>
    ///
    /// <quoted_message …>…</quoted_message>
    ///
    /// <[sender name]: the user's own message, or the forwarded transcript>
    /// ```
    ///
    /// The <recent_context> block is only produced for a group message
    /// addressed to the Bot, and only when recent_context_size > 0 — it
    /// answers MUL-3084 (the Bot saw only the single @-ed line, never the
    /// surrounding conversation). It is the one fetch here NOT triggered by
    /// something the user explicitly attached. When the @-mention arrives
    /// inside a Lark topic (话题) the window is scoped to that topic, so a
    /// topic's context never includes a sibling topic's messages (#5835).
    ///
    /// In group chats, every speaker across ALL blocks (recent + quoted +
    /// forwarded) and the sender who @-mentioned the Bot are resolved to real
    /// display names via ONE Contact batch call, so the agent reads
    /// "[Alice]: …" rather than "[User 1]: …" and knows who addressed it.
    /// This is why the quote/forward items are fetched up front (Phase 1)
    /// before names are resolved (Phase 2). Unresolved senders fall back to
    /// positional "User N"; resolution is best-effort and never blocks. p2p
    /// chats keep positional labels (identity is unambiguous in a 1:1).
    ///
    /// Persistence note: like the quoted/forwarded blocks, the rewritten body
    /// is persisted into the addressed turn's chat_message.content downstream
    /// (AppendUserMessage). Inlining nearby group messages — including ones
    /// from senders who did not address the Bot — into a member's addressed
    /// turn is an accepted product decision for MUL-3084. It does NOT relax
    /// the MUL-2671 drop-audit invariant: a non-addressed group message still
    /// never creates its own session row, and is only ever surfaced as read-
    /// context attached to a turn a workspace member explicitly directed at
    /// the Bot.
    async fn enrich(&self, msg: InboundMessage, creds: InstallationCredentials) -> InboundMessage;
}

/// Tunes the enricher. All fields default.
#[derive(Debug, Clone)]
pub struct InboundEnricherConfig {
    /// Caps inlined forward children. 0 uses DEFAULT_MAX_FORWARD_CHILDREN.
    pub max_forward_children: usize,
    /// Caps how many surrounding group messages the enricher prefetches and
    /// inlines as a <recent_context> block when a user @-mentions the Bot in
    /// a group. 0 DISABLES the prefetch entirely (only explicitly-attached
    /// quote/forward context is used); the production wiring sets
    /// DEFAULT_RECENT_CONTEXT_SIZE. Values above Lark's 50-per-page cap are
    /// clamped by the client.
    pub recent_context_size: i32,
}

impl Default for InboundEnricherConfig {
    fn default() -> Self {
        Self {
            max_forward_children: DEFAULT_MAX_FORWARD_CHILDREN,
            recent_context_size: DEFAULT_RECENT_CONTEXT_SIZE,
        }
    }
}

/// Builds an Enricher backed by the given Lark API client. The client
/// supplies get_message / list_chat_messages / batch_get_users; everything
/// else (flattening, block assembly, speaker labelling) is local.
pub struct InboundEnricher {
    client: Arc<dyn ApiClient>,
    max_forward_children: usize,
    recent_context_size: i32,
}

impl InboundEnricher {
    pub fn new(client: Arc<dyn ApiClient>, cfg: InboundEnricherConfig) -> Self {
        Self {
            client,
            max_forward_children: if cfg.max_forward_children == 0 {
                DEFAULT_MAX_FORWARD_CHILDREN
            } else {
                cfg.max_forward_children
            },
            recent_context_size: cfg.recent_context_size,
        }
    }

    /// Pulls the recent group window and returns the messages to render — the
    /// trigger message itself and the directly-quoted parent (which gets its
    /// own <quoted_message> block) filtered out, sorted oldest-first. A fetch
    /// failure is returned to the caller (which renders a safe, readable
    /// degradation note); it never blocks ingestion.
    ///
    /// When the trigger arrived inside a Lark topic (msg.thread_id != ""),
    /// the window is scoped to that topic (container_id_type=thread) so
    /// sibling topics that share the chat_id can't leak into this topic's
    /// context or its persisted turn (#5835). Because the thread container
    /// rejects end_time, the topic path anchors to the trigger time CLIENT-
    /// side; it also fail-closes on thread_id — any returned item whose
    /// thread_id is missing or does not match is dropped rather than trusted.
    /// A topic fetch failure degrades exactly like the chat path and NEVER
    /// falls back to a chat-wide fetch (that would re-open the leak). Outside
    /// a topic the chat path is unchanged: anchored to the trigger time via
    /// end_time.
    async fn fetch_recent_items(
        &self,
        creds: &InstallationCredentials,
        msg: &InboundMessage,
    ) -> Result<Vec<LarkMessage>, anyhow::Error> {
        let Some(chat_id) = NonEmptyChatId::new(&msg.chat_id) else {
            let err: anyhow::Error = ErrRecentContextChannelUnbound.into();
            let classified = classify_recent_context_fetch_error(&err);
            log_recent_context_fetch_failure(msg, &err, &classified, 0);
            return Err(err);
        };

        // Lark sends create_time as epoch millis; a missing/unparseable time
        // yields 0. The chat path converts it to seconds for end_time; the
        // thread path uses the raw millis for the client-side anchor below.
        let trigger_millis = parse_lark_millis(&msg.create_time);
        let mut params = ListMessagesParams {
            chat_id: ChatId(chat_id.0.to_string()),
            page_size: self.recent_context_size,
            ..ListMessagesParams::default()
        };
        if !msg.thread_id.is_empty() {
            // Topic-scoped fetch: no end_time (the thread container rejects
            // it); the window is anchored client-side below.
            params.thread_id = msg.thread_id.clone();
        } else {
            // 0 tells the client "no end_time" (newest N).
            params.end_time = trigger_millis / 1000;
        }

        let mut items: Vec<LarkMessage> = Vec::new();
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 1..=RECENT_CONTEXT_MAX_FETCH_ATTEMPTS {
            match self
                .client
                .list_chat_messages(creds.clone(), params.clone())
                .await
            {
                Ok(found) => {
                    items = found;
                    if attempt > 1 {
                        tracing::info!(
                            layer = "lark_inbound_enricher",
                            endpoint = RECENT_CONTEXT_ENDPOINT,
                            status = "recovered",
                            attempts = attempt,
                            chat_id = %chat_id.0,
                            message_id = %msg.message_id,
                            "lark enricher: recent context fetch recovered after retry"
                        );
                    }
                    last_err = None;
                    break;
                }
                Err(err) => {
                    let classified = classify_recent_context_fetch_error(&err);
                    // A retry only helps while the shared enrichment budget
                    // still has time left. Both attempts run under one tokio
                    // timeout (ws_connector caps the whole enrich at
                    // enrich_timeout, ~2s), so once the deadline passes the
                    // retry future is dropped mid-flight — degrade now instead
                    // of burning a doomed request.
                    if !classified.retryable || attempt == RECENT_CONTEXT_MAX_FETCH_ATTEMPTS {
                        log_recent_context_fetch_failure(msg, &err, &classified, attempt);
                        return Err(err);
                    }
                    tracing::warn!(
                        layer = "lark_inbound_enricher",
                        endpoint = RECENT_CONTEXT_ENDPOINT,
                        status = "retrying",
                        category = classified.category,
                        retryable = classified.retryable,
                        attempt,
                        next_attempt = attempt + 1,
                        chat_id = %chat_id.0,
                        message_id = %msg.message_id,
                        error = %err,
                        "lark enricher: recent context fetch failed; retrying"
                    );
                    last_err = Some(err);
                }
            }
        }
        if let Some(err) = last_err {
            return Err(err);
        }

        let mut exclude = vec![msg.message_id.clone()];
        if !msg.parent_id.is_empty() {
            exclude.push(msg.parent_id.clone());
        }
        let in_thread = !msg.thread_id.is_empty();
        let mut kept: Vec<LarkMessage> = Vec::with_capacity(items.len());
        for it in items {
            if exclude.contains(&it.message_id) {
                continue;
            }
            // The Bot's markdown replies are sent as schema-2.0 interactive
            // cards, which flatten to a zero-signal "[interactive card]"
            // placeholder — drop them rather than render noise (#5835).
            if it.sender_type == "app" && it.message_type == "interactive" {
                continue;
            }
            if in_thread {
                // Fail-closed topic isolation: the thread container should
                // only return this topic's messages, but if Lark ever returns
                // an item with a missing or mismatched thread_id, drop it
                // rather than risk leaking a sibling topic's content into
                // this topic.
                if it.thread_id != msg.thread_id {
                    continue;
                }
                // The thread container ignores end_time, so anchor
                // client-side: drop anything created strictly after the
                // @-mention moment. A zero trigger time (unparseable)
                // disables the anchor.
                if trigger_millis > 0 && parse_lark_millis(&it.create_time) > trigger_millis {
                    continue;
                }
            }
            kept.push(it);
        }

        // The list endpoint returns newest-first; render oldest-first so the
        // transcript reads top-to-bottom like the chat does.
        kept.sort_by_key(|m| parse_lark_millis(&m.create_time));
        Ok(kept)
    }

    /// Renders the surrounding conversation as a <recent_context> block: one
    /// "[<speaker>]: <text>" line per message, oldest-first, speakers labeled
    /// with real names from `names` (falling back to positional "User N").
    /// Callers pass a non-empty `kept`.
    fn render_recent_context_block(
        &self,
        kept: &[LarkMessage],
        names: &HashMap<String, String>,
    ) -> String {
        let mut labeler = SpeakerLabeler::new(names);
        let lines: Vec<String> = kept
            .iter()
            .map(|m| {
                let label = labeler.label(m);
                let text = if m.message_type == LARK_MSG_TYPE_MERGE_FORWARD {
                    "[merge_forward, expand manually]".to_string()
                } else {
                    match self.flatten_message(m) {
                        t if t.is_empty() => "[empty message]".to_string(),
                        t => t,
                    }
                };
                format!("[{label}]: {text}")
            })
            .collect();
        format!(
            "<recent_context count=\"{}\">\n{}\n</recent_context>",
            kept.len(),
            lines.join("\n")
        )
    }

    /// Renders a <quoted_message> block from the already-fetched
    /// GetMessage(parentID) result. A parent that is itself a merge_forward
    /// nests a <forwarded_messages> transcript inside the quoted block (the
    /// GetMessage response already carries both the forward sentinel and its
    /// children). A fetch error / empty / deleted parent degrades to the
    /// documented error block. Speakers are labeled from `names` (the shared,
    /// already-resolved map), falling back to "User N".
    fn render_quoted_block(
        &self,
        parent_id: &str,
        items: &[LarkMessage],
        err: Option<&anyhow::Error>,
        names: &HashMap<String, String>,
    ) -> String {
        if err.is_some() || items.is_empty() {
            tracing::warn!(
                parent_id,
                items = items.len(),
                error = err.map(|e| e.to_string()).unwrap_or_default(),
                "lark enricher: quoted parent fetch failed"
            );
            return quoted_error_block(parent_id);
        }
        let parent = &items[0];
        if parent.deleted {
            return quoted_error_block(parent_id);
        }

        let mut labeler = SpeakerLabeler::new(names);
        let sender = labeler.label(parent);

        if parent.message_type == LARK_MSG_TYPE_MERGE_FORWARD {
            let inner = self.render_forwarded_items(items, parent_id, names);
            return wrap_quoted(parent_id, &sender, LARK_MSG_TYPE_MERGE_FORWARD, &inner);
        }
        let text = match self.flatten_message(parent) {
            t if t.is_empty() => "[empty message]".to_string(),
            t => t,
        };
        wrap_quoted(parent_id, &sender, &parent.message_type, &text)
    }

    /// Renders the children of a forward whose own record id is forward_id.
    /// Children are time-ordered, capped, and each rendered as
    /// "[<speaker>]: <text>"; a child that is itself a forward is not recursed
    /// into (it gets a manual-expand placeholder) so the HTTP fan-out on the
    /// ACK-latency-sensitive inbound path stays bounded.
    fn render_forwarded_items(
        &self,
        items: &[LarkMessage],
        forward_id: &str,
        names: &HashMap<String, String>,
    ) -> String {
        // The verified contract is that GetMessage(forward_id) returns one
        // level of bundling: [sentinel, direct-children…]. We therefore treat
        // every non-sentinel item as a direct child. We filter by id (not by
        // upper_message_id == forward_id) on purpose: a strict
        // upper_message_id match would silently DROP a real child if Lark
        // ever returned one with that field unpopulated. A child that is
        // itself a forward is rendered as a manual-expand placeholder below
        // rather than recursed into, so grandchildren are never inlined.
        let mut children: Vec<&LarkMessage> = items
            .iter()
            .filter(|it| it.message_id != forward_id)
            .collect();
        let total = children.len();
        if total == 0 {
            return "<forwarded_messages count=\"0\">\n[no forwarded content available]\n</forwarded_messages>"
                .to_string();
        }

        children.sort_by_key(|c| parse_lark_millis(&c.create_time));

        let mut truncated = 0usize;
        if total > self.max_forward_children {
            truncated = total - self.max_forward_children;
            children.truncate(self.max_forward_children);
        }

        let mut labeler = SpeakerLabeler::new(names);
        let lines: Vec<String> = children
            .iter()
            .map(|c| {
                let label = labeler.label(c);
                let text = if c.message_type == LARK_MSG_TYPE_MERGE_FORWARD {
                    "[nested merge_forward, expand manually]".to_string()
                } else {
                    match self.flatten_message(c) {
                        t if t.is_empty() => "[empty message]".to_string(),
                        t => t,
                    }
                };
                format!("[{label}]: {text}")
            })
            .collect();
        let mut body = lines.join("\n");
        if truncated > 0 {
            body.push_str(&format!("\n... ({truncated} more truncated)"));
        }
        format!("<forwarded_messages count=\"{total}\">\n{body}\n</forwarded_messages>")
    }

    /// Turns one fetched message into plain text: structural flatten by
    /// msg_type, then @_user_N placeholder resolution against the message's
    /// own mentions. The bot mention is NOT stripped here (unlike the inbound
    /// decoder) — a quoted / forwarded message is historical context, not a
    /// fresh trigger, so passing empty bot identifiers leaves every @-mention
    /// rendered as a readable @name.
    fn flatten_message(&self, m: &LarkMessage) -> String {
        if m.deleted {
            return "[deleted message]".to_string();
        }
        let raw = flatten_content(&m.message_type, &m.content);
        if raw.is_empty() {
            return String::new();
        }
        resolve_mentions(&raw, &rest_mentions_to_event(&m.mentions), "", "")
    }
}

#[async_trait]
impl Enricher for InboundEnricher {
    #[allow(clippy::too_many_lines)]
    async fn enrich(
        &self,
        mut msg: InboundMessage,
        creds: InstallationCredentials,
    ) -> InboundMessage {
        let fresh_source = if msg.command_body.is_empty() {
            msg.body.clone()
        } else {
            msg.command_body.clone()
        };
        if let Some(body) = cordy_channel_engine::parse_fresh_session_command(&fresh_source) {
            msg.force_fresh_session = true;
            msg.body = body;
        }

        let is_forward = msg.message_type == LARK_MSG_TYPE_MERGE_FORWARD;
        let want_recent = self.recent_context_size > 0
            && msg.chat_type == chat_type_group()
            && msg.addressed_to_bot;
        if msg.parent_id.is_empty() && !is_forward && !want_recent {
            // Nothing to expand and no group prefetch wanted — no network call.
            return msg;
        }
        // If the transport isn't wired (stub client on a deployment without a
        // Lark app), skip rather than stamp every reply with a fetch error.
        // Body stays whatever the decoder produced.
        if !self.client.is_configured() {
            return msg;
        }

        // Phase 1 — fetch every set of messages we may render. Each is
        // best-effort; its error is handled where the block is rendered. We
        // fetch up front (rather than fetch-and-render per block) so Phase 2
        // can resolve display names for EVERY speaker across ALL blocks in a
        // single Contact batch — otherwise a quoted/forwarded sender that
        // isn't in the recent window would fall back to "User N".
        let recent = if want_recent {
            Some(self.fetch_recent_items(&creds, &msg).await)
        } else {
            None
        };
        let quoted = if !msg.parent_id.is_empty() {
            Some(self.client.get_message(creds.clone(), &msg.parent_id).await)
        } else {
            None
        };
        let forward = if is_forward {
            Some(
                self.client
                    .get_message(creds.clone(), &msg.message_id)
                    .await,
            )
        } else {
            None
        };

        // Phase 2 — resolve display names for every speaker we're about to
        // render (recent + quoted + forwarded) plus the sender who @-mentioned
        // the Bot, in one batch. Group chats only; p2p keeps positional labels
        // (identity is unambiguous in a 1:1). Unresolved ids fall back to
        // "User N" per speaker_labeler.
        let empty_names = HashMap::new();
        let names = if msg.chat_type == chat_type_group() {
            let mut ids = Vec::new();
            if let Some(Ok(items)) = recent.as_ref() {
                ids.extend(sender_open_ids(items));
            }
            if let Some(Ok(items)) = quoted.as_ref() {
                ids.extend(sender_open_ids(items));
            }
            if let Some(Ok(items)) = forward.as_ref() {
                ids.extend(sender_open_ids(items));
            }
            if !msg.sender_open_id.is_empty() {
                ids.push(msg.sender_open_id.0.clone());
            }
            self.resolve_names(&creds, &ids).await
        } else {
            empty_names
        };

        // Phase 3 — render broadest-to-narrowest with the complete name map.
        let mut b = String::new();
        if want_recent {
            match recent {
                Some(Err(recent_err)) => {
                    b.push_str(&recent_context_unavailable_line(
                        classify_recent_context_fetch_error(&recent_err).category,
                    ));
                }
                Some(Ok(recent_items)) if !recent_items.is_empty() => {
                    b.push_str(&self.render_recent_context_block(&recent_items, &names));
                }
                _ => {}
            }
        }
        if !msg.parent_id.is_empty() {
            if !b.is_empty() {
                b.push_str("\n\n");
            }
            b.push_str(&self.render_quoted_block(
                &msg.parent_id,
                match &quoted {
                    Some(Ok(items)) => items,
                    _ => &[],
                },
                quoted.as_ref().map(|r| r.as_ref().err()).unwrap_or(None),
                &names,
            ));
        }

        let core = if is_forward {
            match forward {
                Some(Err(forward_err)) => {
                    tracing::warn!(
                        message_id = %msg.message_id,
                        error = %forward_err,
                        "lark enricher: forward fetch failed"
                    );
                    forwarded_error_block()
                }
                Some(Ok(forward_items)) => {
                    self.render_forwarded_items(&forward_items, &msg.message_id, &names)
                }
                None => unreachable!("forward fetch only skipped when !is_forward"),
            }
        } else {
            // Label the user's own message with their real name so the agent
            // knows WHO @-mentioned it — not just what they said. Only when
            // the name resolved (group path); otherwise the body passes
            // through.
            match names.get(&msg.sender_open_id.0) {
                Some(name) if !name.is_empty() => format!("[{}]: {}", name, msg.body),
                _ => msg.body.clone(),
            }
        };
        if !b.is_empty() && !core.is_empty() {
            b.push_str("\n\n");
        }
        b.push_str(&core);

        msg.body = b;
        msg
    }
}

impl InboundEnricher {
    /// Batch-resolves open_ids to display names, best-effort: a failure
    /// (restricted contact scope, transport error) logs and returns an empty
    /// map so every speaker labeler degrades to positional "User N" rather
    /// than blocking ingestion. Duplicate / empty ids are dropped first.
    async fn resolve_names(
        &self,
        creds: &InstallationCredentials,
        ids: &[String],
    ) -> HashMap<String, String> {
        let mut uniq: Vec<String> = Vec::with_capacity(ids.len());
        let mut seen = std::collections::HashSet::with_capacity(ids.len());
        for id in ids {
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            uniq.push(id.clone());
        }
        if uniq.is_empty() {
            return HashMap::new();
        }
        match self.client.batch_get_users(creds.clone(), &uniq).await {
            Ok(names) => names,
            Err(err) => {
                tracing::warn!(
                    ids = uniq.len(),
                    error = %err,
                    "lark enricher: speaker name resolution failed"
                );
                HashMap::new()
            }
        }
    }
}

/// Newtype over a non-empty chat id so the unbound case is unrepresentable in
/// the fetch path.
struct NonEmptyChatId<'a>(&'a str);

impl<'a> NonEmptyChatId<'a> {
    fn new(chat_id: &'a ChatId) -> Option<Self> {
        if chat_id.is_empty() {
            None
        } else {
            Some(Self(&chat_id.0))
        }
    }
}

/// Returns the distinct non-app sender open_ids across the given messages, in
/// first-appearance order — the input set for a Contact name lookup.
fn sender_open_ids(msgs: &[LarkMessage]) -> Vec<String> {
    let mut seen = std::collections::HashSet::with_capacity(msgs.len());
    let mut out = Vec::with_capacity(msgs.len());
    for m in msgs {
        if m.sender_type == "app" || m.sender_id.is_empty() || !seen.insert(m.sender_id.clone()) {
            continue;
        }
        out.push(m.sender_id.clone());
    }
    out
}

fn log_recent_context_fetch_failure(
    msg: &InboundMessage,
    err: &anyhow::Error,
    classified: &RecentContextFetchClassification,
    attempts: usize,
) {
    tracing::warn!(
        layer = "lark_inbound_enricher",
        endpoint = RECENT_CONTEXT_ENDPOINT,
        status = "failed",
        category = classified.category,
        retryable = classified.retryable,
        attempts,
        chat_id = %msg.chat_id,
        message_id = %msg.message_id,
        error = %err,
        "lark enricher: recent context fetch failed"
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentContextFetchClassification {
    pub category: &'static str,
    pub retryable: bool,
}

pub fn classify_recent_context_fetch_error(
    err: &anyhow::Error,
) -> RecentContextFetchClassification {
    if err
        .downcast_ref::<ErrRecentContextChannelUnbound>()
        .is_some()
    {
        return RecentContextFetchClassification {
            category: RECENT_CONTEXT_FAILURE_CHANNEL_UNBOUND,
            retryable: false,
        };
    }
    if let Some(api_err) = err.chain().find_map(|c| c.downcast_ref::<ApiError>()) {
        return classify_recent_context_api_error(api_err.code, &api_err.msg);
    }
    classify_by_message(&err.to_string())
}

fn classify_recent_context_api_error(code: i64, msg: &str) -> RecentContextFetchClassification {
    match code {
        99991002 | 230001 => RecentContextFetchClassification {
            category: RECENT_CONTEXT_FAILURE_PERMISSION_DENIED,
            retryable: false,
        },
        230110 | 230011 | 230050 => RecentContextFetchClassification {
            category: RECENT_CONTEXT_FAILURE_MESSAGE_DELETED,
            retryable: false,
        },
        c if is_token_error(c) => RecentContextFetchClassification {
            category: RECENT_CONTEXT_FAILURE_TOKEN_EXPIRED,
            retryable: true,
        },
        230020 => {
            // See classify_by_message: rate limits degrade rather than retry,
            // since the client drops Retry-After.
            RecentContextFetchClassification {
                category: RECENT_CONTEXT_FAILURE_RATE_LIMITED,
                retryable: false,
            }
        }
        _ => classify_by_message(msg),
    }
}

fn classify_by_message(raw: &str) -> RecentContextFetchClassification {
    let msg = raw.to_lowercase();
    let r = |category: &'static str, retryable: bool| RecentContextFetchClassification {
        category,
        retryable,
    };
    if contains_any(&msg, &["missing chat_id", "missing chat id"]) {
        return r(RECENT_CONTEXT_FAILURE_CHANNEL_UNBOUND, false);
    }
    if contains_any(
        &msg,
        &[
            "code=99991002",
            "code=230001",
            "no permission",
            "permission denied",
            "insufficient permissions",
            "forbidden",
            "http 403",
        ],
    ) {
        return r(RECENT_CONTEXT_FAILURE_PERMISSION_DENIED, false);
    }
    if contains_any(
        &msg,
        &[
            "code=230110",
            "code=230011",
            "code=230050",
            "deleted",
            "recalled",
            "not visible",
            "invisible",
        ],
    ) {
        return r(RECENT_CONTEXT_FAILURE_MESSAGE_DELETED, false);
    }
    if contains_any(&msg, &["code=99991663", "code=99991664"]) {
        return r(RECENT_CONTEXT_FAILURE_TOKEN_EXPIRED, true);
    }
    if contains_any(
        &msg,
        &["code=230020", "rate limit", "rate_limit", "http 429"],
    ) {
        // Not retryable: the client drops Retry-After, and an immediate second
        // call within the same budget almost always re-hits the limit while
        // doubling list load on an already-throttled tenant.
        return r(RECENT_CONTEXT_FAILURE_RATE_LIMITED, false);
    }
    if contains_any(&msg, &["deadline exceeded", "timeout", "timed out"]) {
        return r(RECENT_CONTEXT_FAILURE_TIMEOUT, true);
    }
    if contains_any(
        &msg,
        &[
            "http 500",
            "http 502",
            "http 503",
            "http 504",
            "connection reset",
            "connection refused",
            "temporary",
        ],
    ) {
        return r(RECENT_CONTEXT_FAILURE_TEMPORARY, true);
    }
    r(RECENT_CONTEXT_FAILURE_UNKNOWN, false)
}

fn contains_any(s: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| s.contains(n))
}

/// Renders the degradation line shown in place of a <recent_context> block
/// when the prefetch fails. Category-specific copy keeps operators able to
/// tell a misconfiguration from a blip directly in the persisted turn.
pub fn recent_context_unavailable_line(category: &str) -> String {
    match category {
        RECENT_CONTEXT_FAILURE_CHANNEL_UNBOUND => {
            "[Recent Lark context unavailable: chat binding is missing. Continuing with the latest message.]"
        }
        RECENT_CONTEXT_FAILURE_PERMISSION_DENIED => {
            "[Recent Lark context unavailable: the bot cannot read this chat history. Continuing with the latest message.]"
        }
        RECENT_CONTEXT_FAILURE_MESSAGE_DELETED => {
            "[Recent Lark context unavailable: the referenced chat history is deleted or no longer visible. Continuing with the latest message.]"
        }
        RECENT_CONTEXT_FAILURE_TIMEOUT
        | RECENT_CONTEXT_FAILURE_RATE_LIMITED
        | RECENT_CONTEXT_FAILURE_TOKEN_EXPIRED
        | RECENT_CONTEXT_FAILURE_TEMPORARY => {
            "[Recent Lark context temporarily unavailable; continuing with the latest message.]"
        }
        _ => "[Recent Lark context unavailable; continuing with the latest message.]",
    }
    .to_string()
}

/// Adapts the IM REST mention shape (flat string id) to the WS-event
/// [`LarkMention`] shape resolve_mentions consumes, so a single
/// mention-resolution implementation serves both ingress paths.
pub fn rest_mentions_to_event(ms: &[LarkMessageMention]) -> Vec<LarkMention> {
    ms.iter()
        .map(|m| LarkMention {
            key: m.key.clone(),
            id: LarkMentionId {
                open_id: m.id.clone(),
                union_id: String::new(),
                user_id: String::new(),
            },
            name: m.name.clone(),
        })
        .collect()
}

fn wrap_quoted(message_id: &str, sender: &str, msg_type: &str, inner: &str) -> String {
    format!(
        "<quoted_message message_id=\"{}\" sender=\"{}\" type=\"{}\">\n{}\n</quoted_message>",
        message_id, sender, msg_type, inner
    )
}

fn quoted_error_block(message_id: &str) -> String {
    format!(
        "<quoted_message message_id=\"{}\" type=\"error\">[unable to fetch]</quoted_message>",
        message_id
    )
}

fn forwarded_error_block() -> String {
    "<forwarded_messages type=\"error\">[unable to fetch]</forwarded_messages>".to_string()
}

pub(crate) fn parse_lark_millis(s: &str) -> i64 {
    s.parse::<i64>().unwrap_or(0)
}

/// Assigns stable, human-readable labels to the senders within one rendered
/// block. Lark message items carry only a sender id (no display name in the
/// payload), so the enricher resolves real names out of band via the Contact
/// API and passes them in as a sender-id → name map. A sender present in that
/// map is labeled with their real name; one that is not (restricted contact
/// scope, deactivated user, name lookup failed) falls back to "User 1",
/// "User 2", … in first-appearance order. App senders are always "Bot".
pub(crate) struct SpeakerLabeler<'a> {
    names: &'a HashMap<String, String>,
    seen: HashMap<String, String>,
    n: usize,
}

impl<'a> SpeakerLabeler<'a> {
    pub(crate) fn new(names: &'a HashMap<String, String>) -> Self {
        Self {
            names,
            seen: HashMap::new(),
            n: 0,
        }
    }

    pub(crate) fn label(&mut self, m: &LarkMessage) -> String {
        if m.sender_type == "app" {
            return "Bot".to_string();
        }
        let key = if m.sender_id.is_empty() {
            "unknown"
        } else {
            m.sender_id.as_str()
        };
        if let Some(lbl) = self.seen.get(key) {
            return lbl.clone();
        }
        let lbl = match self.names.get(key) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                self.n += 1;
                format!("User {}", self.n)
            }
        };
        self.seen.insert(key.to_string(), lbl.clone());
        lbl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lark_message(
        id: &str,
        sender_id: &str,
        sender_type: &str,
        create_time: &str,
    ) -> LarkMessage {
        LarkMessage {
            message_id: id.into(),
            sender_id: sender_id.into(),
            sender_type: sender_type.into(),
            create_time: create_time.into(),
            ..Default::default()
        }
    }

    #[test]
    fn sender_open_ids_dedupes_and_skips_apps() {
        let msgs = vec![
            lark_message("om_1", "ou_1", "user", "1"),
            lark_message("om_2", "ou_1", "user", "2"),
            lark_message("om_3", "", "user", "3"),
            lark_message("om_4", "ou_app", "app", "4"),
            lark_message("om_5", "ou_2", "user", "5"),
        ];
        assert_eq!(
            sender_open_ids(&msgs),
            vec!["ou_1".to_string(), "ou_2".to_string()]
        );
    }

    #[test]
    fn classification_maps_api_codes() {
        let perm = ApiError::new("op", 99991002, "denied");
        assert_eq!(
            classify_recent_context_fetch_error(&(perm.into())),
            RecentContextFetchClassification {
                category: RECENT_CONTEXT_FAILURE_PERMISSION_DENIED,
                retryable: false
            }
        );

        let deleted = ApiError::new("op", 230110, "gone");
        assert_eq!(
            classify_recent_context_fetch_error(&(deleted.into())),
            RecentContextFetchClassification {
                category: RECENT_CONTEXT_FAILURE_MESSAGE_DELETED,
                retryable: false
            }
        );

        let token = ApiError::new("op", 99991663, "expired");
        assert!(classify_recent_context_fetch_error(&(token.into())).retryable);

        let rate = ApiError::new("op", 230020, "too many");
        assert_eq!(
            classify_recent_context_fetch_error(&(rate.into())),
            RecentContextFetchClassification {
                category: RECENT_CONTEXT_FAILURE_RATE_LIMITED,
                retryable: false
            }
        );

        let unknown_code = ApiError::new("op", 12345678, "deadline exceeded");
        assert!(classify_recent_context_fetch_error(&(unknown_code.into())).retryable);
    }

    #[test]
    fn classification_maps_transport_strings() {
        let cases = [
            (
                "http do: connection reset",
                RECENT_CONTEXT_FAILURE_TEMPORARY,
                true,
            ),
            (
                "http 500 while listing",
                RECENT_CONTEXT_FAILURE_TEMPORARY,
                true,
            ),
            ("request timeout", RECENT_CONTEXT_FAILURE_TIMEOUT, true),
            (
                "http 403 from lark",
                RECENT_CONTEXT_FAILURE_PERMISSION_DENIED,
                false,
            ),
            (
                "message was recalled",
                RECENT_CONTEXT_FAILURE_MESSAGE_DELETED,
                false,
            ),
            (
                "code=99991664 invalid token",
                RECENT_CONTEXT_FAILURE_TOKEN_EXPIRED,
                true,
            ),
            ("something odd", RECENT_CONTEXT_FAILURE_UNKNOWN, false),
        ];
        for (text, category, retryable) in cases {
            let got = classify_recent_context_fetch_error(&anyhow::anyhow!("{text}"));
            assert_eq!(
                got,
                RecentContextFetchClassification {
                    category,
                    retryable
                },
                "input: {text}"
            );
        }
    }

    #[test]
    fn channel_unbound_sentinel_classifies_directly() {
        let err: anyhow::Error = ErrRecentContextChannelUnbound.into();
        assert_eq!(
            classify_recent_context_fetch_error(&err),
            RecentContextFetchClassification {
                category: RECENT_CONTEXT_FAILURE_CHANNEL_UNBOUND,
                retryable: false
            }
        );
    }

    #[test]
    fn unavailable_lines_match_go_copy() {
        assert_eq!(
            recent_context_unavailable_line(RECENT_CONTEXT_FAILURE_CHANNEL_UNBOUND),
            "[Recent Lark context unavailable: chat binding is missing. Continuing with the latest message.]"
        );
        assert_eq!(
            recent_context_unavailable_line(RECENT_CONTEXT_FAILURE_PERMISSION_DENIED),
            "[Recent Lark context unavailable: the bot cannot read this chat history. Continuing with the latest message.]"
        );
        assert_eq!(
            recent_context_unavailable_line(RECENT_CONTEXT_FAILURE_MESSAGE_DELETED),
            "[Recent Lark context unavailable: the referenced chat history is deleted or no longer visible. Continuing with the latest message.]"
        );
        assert_eq!(
            recent_context_unavailable_line(RECENT_CONTEXT_FAILURE_TIMEOUT),
            "[Recent Lark context temporarily unavailable; continuing with the latest message.]"
        );
        assert_eq!(
            recent_context_unavailable_line(RECENT_CONTEXT_FAILURE_UNKNOWN),
            "[Recent Lark context unavailable; continuing with the latest message.]"
        );
    }

    #[test]
    fn speaker_labeler_prefers_names_then_positional_then_bot() {
        let mut names = HashMap::new();
        names.insert("ou_a".to_string(), "Alice".to_string());
        let mut l = SpeakerLabeler::new(&names);

        let alice = lark_message("om_1", "ou_a", "user", "1");
        let stranger1 = lark_message("om_2", "ou_b", "user", "2");
        let stranger2 = lark_message("om_3", "ou_c", "user", "3");
        let bot = lark_message("om_4", "ou_bot", "app", "4");
        let anon = lark_message("om_5", "", "user", "5");

        assert_eq!(l.label(&alice), "Alice");
        assert_eq!(l.label(&stranger1), "User 1");
        assert_eq!(l.label(&stranger2), "User 2");
        assert_eq!(l.label(&bot), "Bot");
        assert_eq!(l.label(&anon), "User 3");
        // Stable repeats.
        assert_eq!(l.label(&alice), "Alice");
        assert_eq!(l.label(&stranger1), "User 1");
    }

    #[test]
    fn rest_mentions_adapt_to_event_shape() {
        let ms = vec![LarkMessageMention {
            key: "@_user_1".into(),
            id: "ou_9".into(),
            name: "Zoe".into(),
        }];
        let adapted = rest_mentions_to_event(&ms);
        assert_eq!(adapted.len(), 1);
        assert_eq!(adapted[0].id.open_id, "ou_9");
        assert_eq!(adapted[0].name, "Zoe");

        // Resolution through the shared implementation renders the name.
        assert_eq!(resolve_mentions("hi @_user_1", &adapted, "", ""), "hi @Zoe");
    }

    #[test]
    fn parse_lark_millis_tolerates_garbage() {
        assert_eq!(parse_lark_millis("1700000000000"), 1_700_000_000_000);
        assert_eq!(parse_lark_millis(""), 0);
        assert_eq!(parse_lark_millis("abc"), 0);
    }

    #[test]
    fn quoted_and_forwarded_blocks_render_expected_shapes() {
        assert_eq!(
            quoted_error_block("om_p"),
            "<quoted_message message_id=\"om_p\" type=\"error\">[unable to fetch]</quoted_message>"
        );
        assert_eq!(
            forwarded_error_block(),
            "<forwarded_messages type=\"error\">[unable to fetch]</forwarded_messages>"
        );
        assert_eq!(
            wrap_quoted("om_p", "Alice", "text", "hello"),
            "<quoted_message message_id=\"om_p\" sender=\"Alice\" type=\"text\">\nhello\n</quoted_message>"
        );
    }
}
