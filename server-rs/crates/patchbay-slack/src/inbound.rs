//! Platform-neutral translation from a Slack Events API payload to the
//! engine's normalized `patchbay_channel::InboundMessage`. Port of
//!
//! These are free functions parameterized by the bot identity rather than
//! methods on the channel, so the per-installation Socket Mode connection
//! (`channel.rs`) threads in its own installed bot's user id when translating
//! each event.

use patchbay_channel::{ChatType, InboundMessage, MsgType, ReplyCtx, Source};

use crate::raw::{is_fetchable_slack_file_url, SlackRawEvent, SlackRawFile};
use crate::slash_command::SlashCommand;
use crate::TYPE_SLACK;

/// One Slack file object as it appears inside an events payload. Only the
/// fields the translation consumes are modeled; serde ignores the rest.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct EventFile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mimetype: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "url_private", default)]
    pub url_private: String,
    #[serde(rename = "url_private_download", default)]
    pub url_private_download: String,
}

/// The inner event shapes the dispatcher branches on. Both mirror the
/// slack-go structs' field names; unknown fields are ignored.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MessageEvent {
    #[serde(default)]
    pub channel: String,
    /// Slack's own channel classification ("im" / "channel" / "group" /
    /// "mpim" / "private_channel").
    #[serde(rename = "channel_type", default)]
    pub channel_type: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ts: String,
    #[serde(rename = "thread_ts", default)]
    pub thread_ts: String,
    #[serde(default)]
    pub bot_id: String,
    #[serde(default)]
    pub subtype: String,
    /// Brand-new messages nest their full payload here (slack-go Message);
    /// edits carry it at top level instead.
    #[serde(default)]
    pub message: Option<MessageBody>,
    #[serde(default)]
    pub files: Vec<EventFile>,
}

/// The nested message body of a brand-new file_share message: files live on
/// the body, not the envelope.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MessageBody {
    #[serde(default)]
    pub files: Vec<EventFile>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AppMentionEvent {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ts: String,
    #[serde(rename = "thread_ts", default)]
    pub thread_ts: String,
    #[serde(default)]
    pub bot_id: String,
    #[serde(default)]
    pub files: Vec<EventFile>,
}

/// The Events API outer envelope. The inner payload is kept generic and
/// re-parsed per variant by the dispatcher (slack-go decodes
/// `InnerEvent.Data` into concrete types; here serde_json does that job).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct EventsApiEnvelope {
    #[serde(default)]
    pub team_id: String,
    #[serde(rename = "api_app_id", default)]
    pub api_app_id: String,
    #[serde(default)]
    pub event: serde_json::Value,
    /// Socket Mode envelope id — used only for ACK bookkeeping upstream.
    #[serde(default)]
    pub event_id: String,
}

/// Maps the event's file objects to the raw envelope, keeping only files the
/// media resolver could actually fetch: ones with a downloadable URL on a
/// Slack host. That drops uploads still processing (no URL yet) and remote
/// "external" files whose url_private points at Google Drive or Dropbox — the
/// resolver refuses to send the bot token off-domain, so carrying them would
/// only make HasMedia promise media the pipeline can never bind, costing the
/// message a deferred run and an intent-ledger row per doomed download.
fn raw_files_from(files: &[EventFile]) -> Vec<SlackRawFile> {
    let mut out = Vec::new();
    for f in files {
        let mut download_url = f.url_private_download.clone();
        if download_url.is_empty() {
            download_url = f.url_private.clone();
        }
        if !is_fetchable_slack_file_url(&download_url) {
            continue;
        }
        out.push(SlackRawFile {
            id: f.id.clone(),
            name: f.name.clone(),
            mimetype: f.mimetype.clone(),
            size: f.size,
            download_url,
        });
    }
    out
}

/// Builds the regexp that matches an @-mention of bot_user_id. Slack renders a
/// mention as `<@U123>` or `<@U123|name>`. An empty bot_user_id (installation
/// not found / not yet known) yields None — mention detection is then a no-op,
/// which is safe: DMs and app_mention events do not rely on it, and an
/// un-routable team is dropped at installation resolution anyway.
pub fn compile_mention_re(bot_user_id: &str) -> Option<regex::Regex> {
    if bot_user_id.is_empty() {
        return None;
    }
    let quoted = regex::escape(bot_user_id);
    regex::Regex::new(&format!(r"<@{quoted}(\|[^>]*)?>")).ok()
}

struct BuildInboundParams<'a> {
    event_type: &'a str,
    sub_type: &'a str,
    channel_id: &'a str,
    user_id: &'a str,
    text: &'a str,
    ts: &'a str,
    thread_ts: &'a str,
    chat_type: ChatType,
    addressed: bool,
    files: Vec<SlackRawFile>,
}

/// The team + app identity shared by every event on one envelope.
struct EnvelopeIdentity<'a> {
    team_id: &'a str,
    api_app_id: &'a str,
}

fn build_inbound(
    e: &EnvelopeIdentity<'_>,
    p: BuildInboundParams<'_>,
    mention_re: Option<&regex::Regex>,
) -> InboundMessage {
    let raw = serde_json::to_value(SlackRawEvent {
        team_id: e.team_id.to_string(),
        api_app_id: e.api_app_id.to_string(),
        event_type: p.event_type.to_string(),
        subtype: p.sub_type.to_string(),
        channel_type: p.chat_type.0.clone(),
        files: p.files,
    })
    .unwrap_or(serde_json::Value::Null);

    // A thread reply carries thread_ts distinct from its own ts; the root is
    // recoverable from either field.
    let reply = if !p.thread_ts.is_empty() && p.thread_ts != p.ts {
        Some(ReplyCtx {
            message_id: p.thread_ts.to_string(),
            root_id: p.thread_ts.to_string(),
        })
    } else {
        None
    };

    let text = clean_text(p.text, mention_re);
    InboundMessage {
        event_id: p.ts.to_string(),
        message_id: p.ts.to_string(),
        r#type: MsgType::text(),
        text: text.clone(),
        command_text: text,
        media_refs: Vec::new(),
        reply_to: reply,
        addressed_to_bot: p.addressed,
        source: Source {
            channel_type: patchbay_channel::Type(TYPE_SLACK.to_string()),
            chat_id: p.channel_id.to_string(),
            chat_type: p.chat_type,
            sender_id: p.user_id.to_string(),
            sender_stable_id: String::new(),
            thread_id: p.thread_ts.to_string(),
        },
        force_fresh: false,
        skip_agent_run: false,
        raw,
    }
}

/// Converts a verified managed `/agents` slash command into the same
/// provider-neutral envelope as a Socket Mode message. Slack intercepts slash
/// commands before Events API delivery, so the signed command endpoint must
/// explicitly re-enter the shared router for conversation-level Agent routing.
pub fn inbound_from_agents_command(cmd: &SlashCommand) -> Option<InboundMessage> {
    if !cmd.command.trim().eq_ignore_ascii_case("/agents")
        || cmd.trigger_id.trim().is_empty()
        || cmd.team_id.trim().is_empty()
        || cmd.api_app_id.trim().is_empty()
        || cmd.channel_id.trim().is_empty()
        || cmd.user_id.trim().is_empty()
    {
        return None;
    }
    let text = if cmd.text.trim().is_empty() {
        "/agents".to_string()
    } else {
        format!("/agents {}", cmd.text.trim())
    };
    let chat_type = if cmd.channel_id.starts_with('D') {
        ChatType::p2p()
    } else {
        ChatType::group()
    };
    let raw = serde_json::to_value(SlackRawEvent {
        team_id: cmd.team_id.clone(),
        api_app_id: cmd.api_app_id.clone(),
        event_type: "slash_command".to_string(),
        subtype: String::new(),
        channel_type: chat_type.0.clone(),
        files: Vec::new(),
    })
    .unwrap_or(serde_json::Value::Null);
    Some(InboundMessage {
        event_id: cmd.trigger_id.clone(),
        message_id: cmd.trigger_id.clone(),
        r#type: MsgType::text(),
        text: text.clone(),
        command_text: text,
        media_refs: Vec::new(),
        reply_to: None,
        addressed_to_bot: true,
        source: Source {
            channel_type: patchbay_channel::Type(TYPE_SLACK.to_string()),
            chat_id: cmd.channel_id.clone(),
            chat_type,
            sender_id: cmd.user_id.clone(),
            sender_stable_id: String::new(),
            thread_id: String::new(),
        },
        force_fresh: false,
        skip_agent_run: false,
        raw,
    })
}

/// Normalizes a Slack message event. Returns None for events that must not
/// reach the core: the bot's own messages and other bots' messages (loop
/// guard), and edits/deletes/joins and similar subtyped system messages (only
/// brand-new user messages are ingested).
///
/// Group addressing policy (v1, deliberate): a group message is addressed to
/// the bot only when it carries an explicit <@bot> mention. Mention-free
/// follow-ups inside a thread the bot is already engaged in are NOT
/// auto-addressed here: "reply to a bot message" is session state, so it
/// belongs in the session-aware shared service / resolver layer rather than in
/// per-connection adapter memory. Until that lands, channel/thread continuation
/// requires re-mentioning the bot. P2P (DM) ingests every message, unchanged.
pub fn inbound_from_message(
    e: &EventsApiEnvelope,
    m: &MessageEvent,
    bot_user_id: &str,
    mention_re: Option<&regex::Regex>,
) -> Option<InboundMessage> {
    if !m.bot_id.is_empty() || m.subtype == "bot_message" {
        return None;
    }
    if m.user.is_empty() || (!bot_user_id.is_empty() && m.user == bot_user_id) {
        return None;
    }
    if !is_ingestable_subtype(&m.subtype) {
        return None;
    }

    let chat_type = slack_chat_type(&m.channel, &m.channel_type);
    let addressed = chat_type == ChatType::p2p() || mentions_bot(&m.text, mention_re);
    // File attachments live on the nested body for brand-new messages;
    // fall back to the envelope-level list otherwise.
    let files = match &m.message {
        Some(body) if !body.files.is_empty() => raw_files_from(&body.files),
        _ => raw_files_from(&m.files),
    };
    Some(build_inbound(
        &EnvelopeIdentity {
            team_id: &e.team_id,
            api_app_id: &e.api_app_id,
        },
        BuildInboundParams {
            event_type: "message",
            sub_type: &m.subtype,
            channel_id: &m.channel,
            user_id: &m.user,
            text: &m.text,
            ts: &m.ts,
            thread_ts: &m.thread_ts,
            chat_type,
            addressed,
            files,
        },
        mention_re,
    ))
}

/// Normalizes an app_mention event. An app_mention is, by definition, addressed
/// to the bot and occurs in a channel (group). The same channel @mention also
/// arrives as a message event with the identical ts, so the engine's
/// (installation, message_id=ts) dedup collapses the pair — no special-casing
/// needed here.
pub fn inbound_from_app_mention(
    e: &EventsApiEnvelope,
    m: &AppMentionEvent,
    bot_user_id: &str,
    mention_re: Option<&regex::Regex>,
) -> Option<InboundMessage> {
    if !m.bot_id.is_empty()
        || m.user.is_empty()
        || (!bot_user_id.is_empty() && m.user == bot_user_id)
    {
        return None;
    }
    Some(build_inbound(
        &EnvelopeIdentity {
            team_id: &e.team_id,
            api_app_id: &e.api_app_id,
        },
        BuildInboundParams {
            event_type: "app_mention",
            sub_type: "",
            channel_id: &m.channel,
            user_id: &m.user,
            text: &m.text,
            ts: &m.ts,
            thread_ts: &m.thread_ts,
            chat_type: ChatType::group(),
            addressed: true,
            files: raw_files_from(&m.files),
        },
        mention_re,
    ))
}

/// Strips a leading/embedded bot mention token and trims surrounding
/// whitespace so the core sees the user's actual prompt, not "<@U123> hi".
pub fn clean_text(text: &str, mention_re: Option<&regex::Regex>) -> String {
    let text = match mention_re {
        Some(re) => re.replace_all(text, "").into_owned(),
        None => text.to_string(),
    };
    text.trim().to_string()
}

/// Reports whether text contains an @-mention of this bot.
pub fn mentions_bot(text: &str, mention_re: Option<&regex::Regex>) -> bool {
    mention_re.is_some_and(|re| re.is_match(text))
}

/// Maps a Slack channel id / channel_type to the normalized ChatType. Only a
/// 1:1 direct message ("im", or a "D…" channel id) is p2p; everything else —
/// public/private channels AND multi-party DMs ("mpim", which are multi-person
/// conversations) — is a group. A group routes through the engine's "must
/// address the bot" filter, so plain chatter in a multi-party DM is not
/// mistaken for a prompt to the bot.
pub fn slack_chat_type(channel_id: &str, channel_type: &str) -> ChatType {
    match channel_type {
        "im" => return ChatType::p2p(),
        "mpim" | "channel" | "group" | "private_channel" => return ChatType::group(),
        _ => {}
    }
    if channel_id.starts_with('D') {
        return ChatType::p2p();
    }
    ChatType::group()
}

/// Reports whether a message subtype is a brand-new user message the core
/// should ingest. Empty subtype is the normal case; thread_broadcast and
/// file_share are real user messages; everything else (message_changed,
/// message_deleted, channel_join, …) is a system/edit event.
pub fn is_ingestable_subtype(sub_type: &str) -> bool {
    matches!(sub_type, "" | "thread_broadcast" | "file_share")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mention_re() -> Option<regex::Regex> {
        compile_mention_re("U_BOT")
    }

    #[test]
    fn mention_regex_matches_bare_and_labeled_mentions() {
        let re = mention_re().unwrap();
        assert!(re.is_match("<@U_BOT> hi"));
        assert!(re.is_match("<@U_BOT|name> hi"));
        assert!(!re.is_match("<@U_OTHER> hi"));
        assert_eq!(clean_text("<@U_BOT> hi there", Some(&re)), "hi there");
        // Empty bot id disables mention detection entirely.
        assert!(compile_mention_re("").is_none());
    }

    #[test]
    fn chat_type_classification() {
        assert_eq!(slack_chat_type("D123", ""), ChatType::p2p());
        assert_eq!(slack_chat_type("C123", "im"), ChatType::p2p());
        assert_eq!(slack_chat_type("C123", "channel"), ChatType::group());
        assert_eq!(slack_chat_type("G123", "mpim"), ChatType::group());
        assert_eq!(slack_chat_type("C123", ""), ChatType::group());
    }

    #[test]
    fn agents_slash_command_reenters_the_shared_router() {
        let command = SlashCommand {
            command: "/agents".into(),
            text: "2".into(),
            user_id: "U1".into(),
            team_id: "T1".into(),
            api_app_id: "A1".into(),
            channel_id: "C1".into(),
            trigger_id: "trigger-1".into(),
            response_url: String::new(),
        };
        let inbound = inbound_from_agents_command(&command).expect("valid command");
        assert_eq!(inbound.event_id, "trigger-1");
        assert_eq!(inbound.command_text, "/agents 2");
        assert_eq!(inbound.source.chat_id, "C1");
        assert_eq!(inbound.source.chat_type, ChatType::group());
        let raw = crate::raw::decode_slack_raw(&inbound).expect("slack raw");
        assert_eq!(raw.team_id, "T1");
        assert_eq!(raw.api_app_id, "A1");
        assert_eq!(raw.event_type, "slash_command");

        let invalid = SlashCommand {
            trigger_id: String::new(),
            ..command
        };
        assert!(inbound_from_agents_command(&invalid).is_none());
    }

    #[test]
    fn ingestable_subtypes_only_brand_new_messages() {
        assert!(is_ingestable_subtype(""));
        assert!(is_ingestable_subtype("thread_broadcast"));
        assert!(is_ingestable_subtype("file_share"));
        assert!(!is_ingestable_subtype("message_changed"));
        assert!(!is_ingestable_subtype("channel_join"));
    }

    #[test]
    fn drops_bot_and_system_events() {
        let e = EventsApiEnvelope::default();
        let mut m = MessageEvent {
            user: "U1".into(),
            text: "hi".into(),
            ..Default::default()
        };
        assert!(inbound_from_message(&e, &m, "U_BOT", None).is_some());

        m.bot_id = "B1".into();
        assert!(inbound_from_message(&e, &m, "U_BOT", None).is_none());
        m.bot_id = String::new();

        m.subtype = "message_changed".into();
        assert!(inbound_from_message(&e, &m, "U_BOT", None).is_none());
        m.subtype = String::new();

        // The bot's own user id is dropped even without bot_id set.
        m.user = "U_BOT".into();
        assert!(inbound_from_message(&e, &m, "U_BOT", None).is_none());
    }

    #[test]
    fn message_event_builds_envelope_with_raw_files_filtered() {
        let e = EventsApiEnvelope {
            team_id: "T1".into(),
            api_app_id: "A1".into(),
            ..Default::default()
        };
        let m = MessageEvent {
            channel: "C1".into(),
            channel_type: "channel".into(),
            user: "U1".into(),
            text: "<@U_BOT> ship it".into(),
            ts: "1700000000.000100".into(),
            thread_ts: String::new(),
            bot_id: String::new(),
            subtype: String::new(),
            message: Some(MessageBody {
                files: vec![EventFile {
                    id: "F1".into(),
                    name: "x.png".into(),
                    mimetype: "image/png".into(),
                    size: 3,
                    url_private: "https://files.slack.com/files-pri/T1-F1/x.png".into(),
                    url_private_download: String::new(),
                }],
            }),
            files: vec![EventFile {
                id: "F2".into(),
                url_private: "https://drive.google.com/x".into(),
                ..Default::default()
            }],
        };
        let re = mention_re();
        let msg = inbound_from_message(&e, &m, "U_BOT", re.as_ref()).unwrap();
        assert_eq!(msg.source.chat_id, "C1");
        assert_eq!(msg.source.chat_type, ChatType::group());
        assert!(msg.addressed_to_bot);
        assert_eq!(msg.text, "ship it");
        let raw = crate::raw::decode_slack_raw(&msg).unwrap();
        assert_eq!(raw.team_id, "T1");
        assert_eq!(raw.event_type, "message");
        // The Google-Drive-hosted external file was dropped pre-envelope.
        assert_eq!(raw.files.len(), 1);
        assert_eq!(raw.files[0].id, "F1");
    }

    #[test]
    fn dm_message_is_addressed_without_mention() {
        let e = EventsApiEnvelope::default();
        let m = MessageEvent {
            channel: "D123".into(),
            channel_type: "im".into(),
            user: "U1".into(),
            text: "hello".into(),
            ts: "1700000000.000200".into(),
            ..Default::default()
        };
        let msg = inbound_from_message(&e, &m, "U_BOT", None).unwrap();
        assert_eq!(msg.source.chat_type, ChatType::p2p());
        assert!(msg.addressed_to_bot);
        assert!(msg.reply_to.is_none());
    }

    #[test]
    fn threaded_reply_carries_reply_ctx() {
        let e = EventsApiEnvelope::default();
        let m = MessageEvent {
            channel: "C1".into(),
            channel_type: "channel".into(),
            user: "U1".into(),
            text: "<@U_BOT> follow up".into(),
            ts: "1700000000.000300".into(),
            thread_ts: "1699999000.000000".into(),
            ..Default::default()
        };
        let re = mention_re();
        let msg = inbound_from_message(&e, &m, "U_BOT", re.as_ref()).unwrap();
        let reply = msg.reply_to.expect("threaded reply");
        assert_eq!(reply.message_id, "1699999000.000000");
        assert_eq!(reply.root_id, "1699999000.000000");
        assert_eq!(msg.source.thread_id, "1699999000.000000");
    }

    #[test]
    fn app_mention_is_always_addressed_group_event() {
        let e = EventsApiEnvelope {
            team_id: "T1".into(),
            api_app_id: "A1".into(),
            ..Default::default()
        };
        let m = AppMentionEvent {
            channel: "C1".into(),
            user: "U1".into(),
            text: "<@U_BOT> /issue fix".into(),
            ts: "1700000000.000400".into(),
            ..Default::default()
        };
        let re = mention_re();
        let msg = inbound_from_app_mention(&e, &m, "U_BOT", re.as_ref()).unwrap();
        assert!(msg.addressed_to_bot);
        assert_eq!(msg.source.chat_type, ChatType::group());
        assert_eq!(msg.text, "/issue fix");
        let raw = crate::raw::decode_slack_raw(&msg).unwrap();
        assert_eq!(raw.event_type, "app_mention");

        // Bot-authored app_mentions are dropped.
        let m = AppMentionEvent {
            bot_id: "B1".into(),
            ..m
        };
        assert!(inbound_from_app_mention(&e, &m, "U_BOT", None).is_none());
    }
}
