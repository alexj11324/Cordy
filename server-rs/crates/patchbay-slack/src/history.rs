//! On-demand Slack conversation history — the pull side of the unified
//! `patchbay chat history` (channel overview) and `patchbay chat thread [id]` (one
//! thread) commands.

use sqlx::PgPool;
use uuid::Uuid;

use patchbay_channel::{HistoryMessage, HistoryOptions, HistoryPage, HistoryRole};
use patchbay_db::models::{ChannelChatSessionBinding, ChannelInstallation};
use patchbay_db::queries::channel::{
    get_channel_chat_session_binding_by_session, get_channel_installation,
};

use crate::client::{ConversationHistoryResponse, Message as SlackMessage, SlackClient};
use crate::config::{decode_credentials, Decrypter};
use crate::resolvers::SlackBindingConfig;
use crate::TYPE_SLACK;

/// Reports that the chat session has no Slack channel binding — it is a Feishu
/// or web-only session. Callers surface it as an empty (not failed) read so the
/// unified commands answer gracefully on a non-Slack conversation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("slack: session has no slack channel binding")]
pub struct ErrNoSlackSession;

/// The page size used when the caller asks for none.
const DEFAULT_HISTORY_LIMIT: i64 = 20;
/// Caps a single page so a pull can't dump an unbounded Agent event history into the
/// agent's context.
const MAX_HISTORY_LIMIT: i64 = 50;

/// Caps text recovered from attachments/blocks so a verbose alert card cannot
/// flood the agent's context. It applies only to the fallback path; a normal
/// top-level message body is passed through untouched.
const MAX_DERIVED_TEXT_LEN: usize = 4000;

/// Reads a Slack conversation on demand. Both reads are scoped to the
/// session's OWN channel: the channel is resolved server-side from the binding
/// and never taken from the agent, so a thread id is only a within-channel
/// locator. Sessions with no Slack binding return
/// [`ErrNoSlackSession`].
pub struct History {
    pool: PgPool,
    decrypt: Option<std::sync::Arc<Decrypter>>,
}

impl History {
    /// Builds the reader over the pool and the bot-token decrypter (box.Open
    /// at wiring time).
    pub fn new(pool: PgPool, decrypt: Option<std::sync::Arc<Decrypter>>) -> Self {
        Self { pool, decrypt }
    }

    /// Maps a chat_session to its Slack channel + bot client. The channel is
    /// server-derived here and never accepted from the caller — that is the
    /// security boundary for `patchbay chat thread <id>` (the agent supplies only
    /// a within-channel thread locator).
    async fn resolve(&self, chat_session_id: Uuid) -> anyhow::Result<SlackTarget> {
        let binding =
            get_channel_chat_session_binding_by_session(&self.pool, chat_session_id, TYPE_SLACK)
                .await?
                .ok_or(ErrNoSlackSession)?;
        let inst = get_channel_installation(&self.pool, binding.installation_id, TYPE_SLACK)
            .await?
            .ok_or(ErrNoSlackSession)?;
        if inst.status != "active" {
            return Err(ErrNoSlackSession.into()); // revoked install: nothing to read
        }
        let creds = decode_credentials(&inst.config, self.decrypt.as_deref())
            .map_err(|e| anyhow::anyhow!("decode slack credentials: {e}"))?;
        let (channel_id, thread_root) = history_target(&binding);
        Ok(SlackTarget {
            client: SlackClient::new(creds.bot_token),
            channel_id,
            // The session's own thread (empty for a DM)
            thread_root,
            bot_user_id: creds.bot_user_id,
        })
    }

    /// Returns the channel's recent top-level messages (oldest-first), each
    /// thread tagged with its id + reply count. It does NOT expand thread
    /// contents — it is the table of contents the agent reads to find a
    /// thread, then drills into with `patchbay chat thread <id>`. Backs
    /// `patchbay chat history`.
    pub async fn channel_overview(
        &self,
        chat_session_id: Uuid,
        opts: &HistoryOptions,
    ) -> anyhow::Result<HistoryPage> {
        let t = self.resolve(chat_session_id).await?;
        let limit = clamp_history_limit(opts.limit);
        let resp = t
            .client
            .conversations_history(&t.channel_id, &opts.before, limit)
            .await
            .map_err(|e| anyhow::anyhow!("read slack channel: {e}"))?;
        let mut page = normalize_page(&t.client, &resp.messages, &t.bot_user_id, limit, true).await;
        page.channel_type = TYPE_SLACK.to_string();
        Ok(page)
    }

    /// Returns one thread's messages (oldest-first). thread_id empty reads the
    /// thread the session is in (the agent's own thread); a non-empty id reads
    /// that specific thread — but always within the session's pinned channel.
    /// A DM (no threads) reads its linear conversation. Backs
    /// `patchbay chat thread [id]`.
    pub async fn thread(
        &self,
        chat_session_id: Uuid,
        thread_id: &str,
        opts: &HistoryOptions,
    ) -> anyhow::Result<HistoryPage> {
        let t = self.resolve(chat_session_id).await?;
        let limit = clamp_history_limit(opts.limit);
        let ts = if thread_id.is_empty() {
            t.thread_root.clone() // the session's own thread
        } else {
            thread_id.to_string()
        };

        let raw: Vec<SlackMessage> = if ts.is_empty() {
            // No thread to read (a DM, or a group whose root could not be
            // recovered): fall back to the channel's linear conversation.
            let resp: ConversationHistoryResponse = t
                .client
                .conversations_history(&t.channel_id, &opts.before, limit)
                .await
                .map_err(|e| anyhow::anyhow!("read slack thread: {e}"))?;
            resp.messages
        } else {
            t.client
                .conversations_replies(&t.channel_id, &ts, &opts.before, limit)
                .await
                .map_err(|e| anyhow::anyhow!("read slack thread: {e}"))?
                .messages
        };
        let mut page = normalize_page(&t.client, &raw, &t.bot_user_id, limit, false).await;
        page.channel_type = TYPE_SLACK.to_string();
        page.thread_id = ts;
        Ok(page)
    }
}

/// The resolved per-session read context: a bot-token client plus the
/// session's pinned channel and its own thread root.
struct SlackTarget {
    client: SlackClient,
    channel_id: String,
    thread_root: String,
    bot_user_id: String,
}

fn clamp_history_limit(n: i64) -> i64 {
    if n <= 0 {
        return DEFAULT_HISTORY_LIMIT;
    }
    n.min(MAX_HISTORY_LIMIT)
}

/// Recovers the real channel id and the session's own thread root from the
/// binding. The channel_chat_id may be a composite "channel:threadRoot"
/// isolation key, so the real channel id is read from the binding config
/// ([`SlackBindingConfig`]). The thread root is the recorded reply thread
/// (last_thread_id), falling back to the composite-key suffix; empty for a DM.
fn history_target(b: &ChannelChatSessionBinding) -> (String, String) {
    let mut channel_id = b.channel_chat_id.clone();
    if !b.config.is_null() {
        if let Ok(cfg) = serde_json::from_value::<SlackBindingConfig>(b.config.clone()) {
            if !cfg.channel_id.is_empty() {
                channel_id = cfg.channel_id;
            }
        }
    }
    let thread_root = match &b.last_thread_id {
        Some(s) if !s.is_empty() => s.clone(),
        _ => match b.channel_chat_id.find(':') {
            Some(i) => b.channel_chat_id[i + 1..].to_string(),
            None => String::new(),
        },
    };
    (channel_id, thread_root)
}

/// Turns raw Slack messages into a normalized, oldest-first page: it resolves
/// display names in one batch, labels senders, maps roles, and computes the
/// back-paging cursor. When overview is true, a message that heads a thread
/// (reply_count > 0) is tagged with its thread id + reply count so the agent
/// can drill in with `patchbay chat thread <id>`.
async fn normalize_page(
    client: &SlackClient,
    raw: &[SlackMessage],
    bot_user_id: &str,
    limit: i64,
    overview: bool,
) -> HistoryPage {
    let mut raw: Vec<SlackMessage> = raw.to_vec();
    raw.sort_by(|a, b| slack_ts_less(&a.ts, &b.ts));

    let names = resolve_user_names(client, &raw, bot_user_id).await;
    let mut labeler = HistoryLabeler::new(names);

    let mut out = Vec::with_capacity(raw.len());
    for m in &raw {
        let text = flatten_slack_text(m);
        if text.is_empty() {
            continue; // genuine join/system/edit marker: no readable body
        }
        let own = !m.user.is_empty() && m.user == bot_user_id;
        let role = if own {
            HistoryRole::assistant()
        } else {
            HistoryRole::user()
        };
        let mut hm = HistoryMessage {
            id: m.ts.clone(),
            author: labeler.label(m, own),
            author_id: m.user.clone(),
            role,
            text,
            ts: m.ts.clone(),
            thread_id: String::new(),
            reply_count: 0,
            latest_reply: String::new(),
        };
        if overview && m.reply_count > 0 {
            hm.thread_id = m.ts.clone();
            hm.reply_count = m.reply_count;
            hm.latest_reply = m.latest_reply.clone();
        }
        out.push(hm);
    }

    let mut page = HistoryPage {
        channel_type: String::new(),
        thread_id: String::new(),
        next_cursor: String::new(),
        messages: out,
    };
    // Advertise a cursor only when the platform returned a full page (more may
    // exist older than the oldest message we just returned).
    if raw.len() as i64 >= limit && !page.messages.is_empty() {
        page.next_cursor = page.messages[0].ts.clone();
    }
    page
}

/// Renders a Slack message to the plain-text body the history contract
/// promises. Alerting/webhook bots (Grafana cards, incoming webhooks) carry
/// their whole body in attachments or Block Kit blocks and leave the top-level
/// Text empty; without this fallback such a message is indistinguishable from
/// a join/system marker and gets dropped (PB-3931 / #4803). Order: top-level
/// text, then each attachment's rendered text/fields, then last-resort
/// fallback text, then a best-effort blocks flatten. Returns "" only when
/// nothing renderable exists — a real system marker.
fn flatten_slack_text(m: &SlackMessage) -> String {
    let t = m.text.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    let mut parts: Vec<String> = Vec::with_capacity(m.attachments.len() + 1);
    for a in &m.attachments {
        if let Some(t) = attachment_text(a) {
            parts.push(t);
        }
    }
    if parts.is_empty() {
        if let Some(t) = flatten_blocks(&m.blocks) {
            parts.push(t);
        }
    }
    truncate_runes(parts.join("\n").trim(), MAX_DERIVED_TEXT_LEN)
}

/// Summarizes one attachment. Attachment fallback is only a last-resort summary
/// for clients that cannot render attachments; Grafana-style alerts often put
/// the useful alert body in Text/Fields while Fallback repeats the short title.
fn attachment_text(a: &crate::client::Attachment) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(3 + a.fields.len());
    for s in [&a.pretext, &a.title, &a.text] {
        let s = s.trim();
        if !s.is_empty() {
            parts.push(s.to_string());
        }
    }
    for f in &a.fields {
        let combined = format!("{} {}", f.title.trim(), f.value.trim());
        let combined = combined.trim();
        if !combined.is_empty() {
            parts.push(combined.to_string());
        }
    }
    if !parts.is_empty() {
        return Some(parts.join("\n"));
    }
    let fallback = a.fallback.trim();
    if !fallback.is_empty() {
        return Some(fallback.to_string());
    }
    flatten_blocks(&a.blocks)
}

/// Renders Block Kit blocks to plain text, best-effort: it walks the common
/// text-bearing blocks (section, header, context, markdown, and rich_text) and
/// skips interactive/media blocks.
fn flatten_blocks(blocks: &[crate::client::Block]) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(blocks.len());
    let mut add = |s: &str| {
        let s = s.trim();
        if !s.is_empty() {
            parts.push(s.to_string());
        }
    };
    for b in blocks {
        match b {
            crate::client::Block::Section { text, fields } => {
                if let Some(t) = text {
                    add(&t.text);
                }
                for f in fields {
                    add(&f.text);
                }
            }
            crate::client::Block::Header { text } => {
                if let Some(t) = text {
                    add(&t.text);
                }
            }
            crate::client::Block::Markdown { text } => add(text),
            crate::client::Block::Context { elements } => {
                for el in elements {
                    match el {
                        crate::client::ContextElement::PlainText { text }
                        | crate::client::ContextElement::Mrkdwn { text } => add(text),
                        crate::client::ContextElement::Image { .. } => {}
                        crate::client::ContextElement::Other => {}
                    }
                }
            }
            crate::client::Block::RichText { elements } => {
                add(&rich_text_block_text(elements));
            }
            crate::client::Block::Other => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Flattens a rich_text block to plain text, best-effort: it walks sections,
/// lists, quotes, and preformatted runs and concatenates their text and link
/// runs (one line per section). Mentions, emoji, and other inline decorations
/// are skipped — this is the plain body an agent needs, not a faithful
/// re-render. A rich_text-only body is the standard shape for messages composed
/// in Slack's own rich text input, so a bot that posts one with an empty
/// top-level Text would otherwise be dropped.
fn rich_text_block_text(elements: &[crate::client::RichTextElement]) -> String {
    fn write_section(els: &[crate::client::RichTextSectionElement], lines: &mut Vec<String>) {
        let mut sb = String::new();
        for e in els {
            match e {
                crate::client::RichTextSectionElement::Text { text } => sb.push_str(text),
                crate::client::RichTextSectionElement::Link { text, url } => {
                    if !text.is_empty() {
                        sb.push_str(text);
                    } else {
                        sb.push_str(url);
                    }
                }
                crate::client::RichTextSectionElement::Other => {}
            }
        }
        let s = sb.trim();
        if !s.is_empty() {
            lines.push(s.to_string());
        }
    }

    fn write_element(el: &crate::client::RichTextElement, lines: &mut Vec<String>) {
        use crate::client::RichTextElement::*;
        match el {
            RichTextSection { elements }
            | RichTextQuote { elements }
            | RichTextPreformatted { elements } => write_section(elements, lines),
            RichTextList { elements } => {
                for item in elements {
                    write_element(item, lines);
                }
            }
            Other => {}
        }
    }

    let mut lines: Vec<String> = Vec::new();
    for el in elements {
        write_element(el, &mut lines);
    }
    lines.join("\n")
}

/// Trims s to at most max runes, appending an ellipsis when cut.
fn truncate_runes(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// Batch-resolves human senders' display names, best-effort. A failure
/// (missing users:read scope, transport error) yields an empty map so the
/// labeler falls back to positional "User N" rather than blocking the read.
async fn resolve_user_names(
    client: &SlackClient,
    msgs: &[SlackMessage],
    bot_user_id: &str,
) -> std::collections::HashMap<String, String> {
    let mut seen = std::collections::HashSet::new();
    let mut ids: Vec<String> = Vec::new();
    for m in msgs {
        let u = m.user.as_str();
        if u.is_empty() || u == bot_user_id || seen.contains(u) {
            continue;
        }
        seen.insert(u.to_string());
        ids.push(u.to_string());
    }
    if ids.is_empty() {
        return std::collections::HashMap::new();
    }
    let users = match client.users_info(&ids).await {
        Ok(users) => users,
        Err(e) => {
            tracing::warn!(ids = ids.len(), error = %e, "slack history: user name resolution failed");
            return std::collections::HashMap::new();
        }
    };
    let mut names = std::collections::HashMap::with_capacity(users.len());
    for u in users {
        if let Some(name) = slack_display_name(&u) {
            names.insert(u.id, name);
        }
    }
    names
}

/// Picks the friendliest available name for a Slack user.
fn slack_display_name(u: &crate::client::User) -> Option<String> {
    if !u.profile.display_name.is_empty() {
        return Some(u.profile.display_name.clone());
    }
    if !u.real_name.is_empty() {
        return Some(u.real_name.clone());
    }
    if !u.name.is_empty() {
        return Some(u.name.clone());
    }
    None
}

/// Assigns stable, human-readable labels within one page: this bot is "Bot"; a
/// resolved human gets their real name; an unresolved human falls back to
/// positional "User N"; a third-party bot uses its posted username.
struct HistoryLabeler {
    names: std::collections::HashMap<String, String>,
    seen: std::collections::HashMap<String, String>,
    n: usize,
}

impl HistoryLabeler {
    fn new(names: std::collections::HashMap<String, String>) -> Self {
        Self {
            names,
            seen: std::collections::HashMap::new(),
            n: 0,
        }
    }

    fn label(&mut self, m: &SlackMessage, own: bool) -> String {
        if own {
            return "Bot".to_string();
        }
        let key = if m.user.is_empty() {
            if !m.username.is_empty() {
                return m.username.clone();
            }
            format!("bot:{}", m.bot_id)
        } else {
            m.user.clone()
        };
        if let Some(lbl) = self.seen.get(&key) {
            return lbl.clone();
        }
        let lbl = if let Some(name) = self.names.get(&m.user) {
            name.clone()
        } else if !m.username.is_empty() {
            m.username.clone()
        } else {
            self.n += 1;
            format!("User {}", self.n)
        };
        self.seen.insert(key, lbl.clone());
        lbl
    }
}

/// Orders two Slack timestamps ("secs.micros") chronologically.
fn slack_ts_less(a: &str, b: &str) -> std::cmp::Ordering {
    parse_slack_ts(a)
        .partial_cmp(&parse_slack_ts(b))
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn parse_slack_ts(ts: &str) -> f64 {
    ts.parse::<f64>().unwrap_or(0.0)
}

// Referenced by the media resolver's shared model docs; keeps the import of
// ChannelInstallation meaningful on non-DB builds too.
#[allow(dead_code)]
type InstallationRow = ChannelInstallation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_history_limits() {
        assert_eq!(clamp_history_limit(0), DEFAULT_HISTORY_LIMIT);
        assert_eq!(clamp_history_limit(-5), DEFAULT_HISTORY_LIMIT);
        assert_eq!(clamp_history_limit(10), 10);
        assert_eq!(clamp_history_limit(10_000), MAX_HISTORY_LIMIT);
    }

    #[test]
    fn history_target_reads_channel_from_config_and_root_from_key_suffix() {
        let b = ChannelChatSessionBinding {
            channel_chat_id: "C1:T_ROOT".to_string(),
            channel_type: TYPE_SLACK.to_string(),
            chat_session_id: Uuid::nil(),
            chat_type: "group".to_string(),
            config: serde_json::json!({"channel_id": "C_REAL"}),
            created_at: chrono::Utc::now(),
            id: Uuid::nil(),
            installation_id: Uuid::nil(),
            last_message_id: None,
            last_thread_id: None,
            pending_fresh: false,
        };
        let (channel, root) = history_target(&b);
        assert_eq!(channel, "C_REAL");
        assert_eq!(root, "T_ROOT");

        // No recorded thread + plain key → empty root (DM).
        let dm = ChannelChatSessionBinding {
            channel_chat_id: "D1".to_string(),
            config: serde_json::json!({}),
            ..b
        };
        let (channel, root) = history_target(&dm);
        assert_eq!(channel, "D1");
        assert_eq!(root, "");

        // last_thread_id wins over the key suffix.
        let threaded = ChannelChatSessionBinding {
            channel_chat_id: "C1:OLD".to_string(),
            config: serde_json::json!({"channel_id": "C_REAL"}),
            last_thread_id: Some("1699999000.5".to_string()),
            ..dm
        };
        let (_, root) = history_target(&threaded);
        assert_eq!(root, "1699999000.5");
    }

    #[test]
    fn ts_ordering_is_numeric_not_lexicographic() {
        // 0.9 < 0.10 numerically but not lexicographically.
        assert_eq!(
            slack_ts_less("1700000000.09", "1700000000.10"),
            std::cmp::Ordering::Less
        );
        assert_eq!(slack_ts_less("99", "1700000000"), std::cmp::Ordering::Less);
        // Malformed timestamps sort as zero.
        assert_eq!(slack_ts_less("junk", "1"), std::cmp::Ordering::Less);
    }

    #[test]
    fn display_name_prefers_profile_then_real_name_then_username() {
        let mk = |display: &str, real: &str, name: &str| crate::client::User {
            profile: crate::client::UserProfile {
                display_name: display.to_string(),
            },
            real_name: real.to_string(),
            name: name.to_string(),
            ..Default::default()
        };
        assert_eq!(
            slack_display_name(&mk("disp", "real", "user")).as_deref(),
            Some("disp")
        );
        assert_eq!(
            slack_display_name(&mk("", "real", "user")).as_deref(),
            Some("real")
        );
        assert_eq!(
            slack_display_name(&mk("", "", "user")).as_deref(),
            Some("user")
        );
        assert_eq!(slack_display_name(&mk("", "", "")), None);
    }

    #[test]
    fn labeler_assigns_bot_positional_and_resolved_names_stably() {
        let mut l = HistoryLabeler::new(
            [("U1".to_string(), "Alice".to_string())]
                .into_iter()
                .collect(),
        );
        let own_msg = SlackMessage {
            user: "U_BOT".to_string(),
            ..Default::default()
        };
        assert_eq!(l.label(&own_msg, true), "Bot");

        let resolved = SlackMessage {
            user: "U1".to_string(),
            ..Default::default()
        };
        assert_eq!(l.label(&resolved, false), "Alice");

        let unnamed_bot = SlackMessage {
            username: "Grafana".to_string(),
            bot_id: "B9".to_string(),
            ..Default::default()
        };
        assert_eq!(l.label(&unnamed_bot, false), "Grafana");

        let unknown_a = SlackMessage {
            user: "U7".to_string(),
            ..Default::default()
        };
        assert_eq!(l.label(&unknown_a, false), "User 1");
        // Same sender keeps the same label within the page.
        assert_eq!(l.label(&unknown_a, false), "User 1");
    }

    #[test]
    fn flatten_falls_back_to_attachments_and_blocks() {
        // Top-level text wins untouched.
        let with_text = SlackMessage {
            text: " body ".to_string(),
            attachments: vec![crate::client::Attachment {
                pretext: "ignored".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(flatten_slack_text(&with_text), "body");

        // Empty body falls back to attachment rendering.
        let att_only = SlackMessage {
            attachments: vec![crate::client::Attachment {
                pretext: "ALERT".to_string(),
                title: "CPU high".to_string(),
                fields: vec![crate::client::AttachmentField {
                    title: "host".to_string(),
                    value: "web-1".to_string(),
                }],
                fallback: "short title".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(flatten_slack_text(&att_only), "ALERT\nCPU high\nhost web-1");

        // Blocks are the last resort; rich text link runs render their label.
        let block_only = SlackMessage {
            blocks: vec![crate::client::Block::RichText {
                elements: vec![crate::client::RichTextElement::RichTextSection {
                    elements: vec![
                        crate::client::RichTextSectionElement::Text {
                            text: "see ".to_string(),
                        },
                        crate::client::RichTextSectionElement::Link {
                            text: "docs".to_string(),
                            url: "https://x.example".to_string(),
                        },
                    ],
                }],
            }],
            ..Default::default()
        };
        assert_eq!(flatten_slack_text(&block_only), "see docs");

        // Nothing renderable → empty string (system marker).
        assert_eq!(flatten_slack_text(&SlackMessage::default()), "");
    }

    #[test]
    fn truncate_runes_appends_ellipsis_on_cut() {
        assert_eq!(truncate_runes("abc", 10), "abc");
        assert_eq!(truncate_runes("abcdef", 3), "abc…");
        // Rune boundaries respected (é is one rune, two bytes).
        assert_eq!(truncate_runes("ééé", 2), "éé…");
    }
}
