//! The aibot WebSocket wire format — port of `ws_frame.go`.
//!
//! Every frame is JSON with a `{cmd, headers.req_id, body}` envelope. We
//! only parse the frames we act on:
//!
//! - inbound  — aibot_msg_callback (user message), aibot_event_callback (event)
//! - outbound — aibot_subscribe (auth), ping (heartbeat), aibot_send_msg
//!   (push), aibot_respond_msg (in-window reply)
//! - response — the ack the server writes for aibot_subscribe / ping / send_msg
//!
//! The wire is documented at
//! <https://developer.work.weixin.qq.com/document/path/101463>.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use patchbay_channel::message::{
    ChatType, InboundMessage as ChannelInboundMessage, MsgType, Source,
};

use crate::type_wecom;

// Frame commands the client sends.
pub const CMD_SUBSCRIBE: &str = "aibot_subscribe";
pub const CMD_PING: &str = "ping";
pub const CMD_SEND_MSG: &str = "aibot_send_msg";
pub const CMD_RESPOND_MSG: &str = "aibot_respond_msg";

// Frame commands the server sends. These are what the read loop switches on.
pub const CMD_MSG_CALLBACK: &str = "aibot_msg_callback";
pub const CMD_EVENT_CALLBACK: &str = "aibot_event_callback";
pub const CMD_SERVER_PING: &str = "ping";
pub const CMD_PONG: &str = "pong";

// Event types inside aibot_event_callback.body.event.eventtype.
pub const EVENT_DISCONNECTED: &str = "disconnected_event";
pub const EVENT_ENTER_CHAT: &str = "enter_chat";
pub const EVENT_TEMPLATE_CARD: &str = "template_card_event";
pub const EVENT_FEEDBACK: &str = "feedback_event";

/// aibot receiver kind for aibot_send_msg: a single (1:1) user.
pub const CHAT_TYPE_SINGLE_INT: i64 = 1;
/// aibot receiver kind for aibot_send_msg: a group chat.
pub const CHAT_TYPE_GROUP_INT: i64 = 2;

/// Carries a per-frame correlation id. Server acks reflect the req_id back so
/// the client can pair requests with responses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameHeaders {
    #[serde(default)]
    pub req_id: String,
}

/// The outer shape of every frame the server pushes. Body is left raw so
/// downstream code can unmarshal the specific shape without re-parsing the
/// outer wrapper.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FrameEnvelope {
    #[serde(default)]
    pub cmd: String,
    #[serde(default)]
    pub headers: FrameHeaders,
    #[serde(default)]
    pub body: Value,

    // Response fields (present when the server acks one of our writes). The
    // wire spells them "errcode"/"errmsg".
    #[serde(default, alias = "errcode")]
    pub err_code: i64,
    #[serde(default, alias = "errmsg")]
    pub err_msg: String,
}

/// The `{url, aeskey}` pair every downloadable kind carries. In
/// long-connection mode the key is minted per url, so it lives on the message
/// rather than in configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaBody {
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "aeskey")]
    pub aes_key: String,
}

/// One run of a 图文混排 message: a sentence, a spoken sentence, or an
/// attachment, in the order the user composed them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MixedItem {
    #[serde(default, rename = "msgtype")]
    pub msg_type: String,
    #[serde(default)]
    pub text: TextBody,
    #[serde(default)]
    pub voice: TextBody,
    #[serde(default)]
    pub image: MediaBody,
    #[serde(default)]
    pub file: MediaBody,
    #[serde(default)]
    pub video: MediaBody,
}

/// A `{content}` sub-object shared by the text and voice bodies.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextBody {
    #[serde(default)]
    pub content: String,
}

/// The body of an aibot_msg_callback frame — a user message pushed from a
/// chat to the bot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AibotMsgCallback {
    #[serde(default, rename = "msgid")]
    pub msg_id: String,
    #[serde(default, rename = "aibotid")]
    pub ai_bot_id: String,
    #[serde(default, rename = "chatid")]
    pub chat_id: String,
    /// `"single"` | `"group"`.
    #[serde(default, rename = "chattype")]
    pub chat_type: String,
    #[serde(default)]
    pub from: FromUser,
    /// `"text" | "image" | "voice" | "file" | "video" | "mixed"`.
    #[serde(default, rename = "msgtype")]
    pub msg_type: String,
    #[serde(default)]
    pub text: TextBody,
    /// Voice carries the TRANSCRIPT, not audio. WeCom runs the speech
    /// recognition on its side and delivers only the result, so a voice note
    /// needs no download, no media key and no storage — it is a sentence that
    /// happened to be spoken. Not gated on chat type: whatever chat a voice
    /// note arrives from, the transcript is read the same way.
    #[serde(default)]
    pub voice: TextBody,
    /// Image / File / Video are the downloadable kinds. Each carries only a
    /// pre-signed COS url and the key its bytes are encrypted with — no name,
    /// no size, no MIME type (see media_download.rs for where those come from
    /// instead).
    #[serde(default)]
    pub image: MediaBody,
    #[serde(default)]
    pub file: MediaBody,
    #[serde(default)]
    pub video: MediaBody,
    /// Mixed carries 图文混排 — a message the user composed with text runs
    /// and attachments interleaved. Each item is itself typed and carries the
    /// same bodies a standalone message of that type would.
    #[serde(default)]
    pub mixed: MixedBody,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FromUser {
    #[serde(default, rename = "userid")]
    pub user_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MixedBody {
    #[serde(default, rename = "msg_item")]
    pub msg_item: Vec<MixedItem>,
}

impl MixedItem {
    /// The part of one 图文混排 run the SENDER typed or said. An attachment
    /// contributes nothing, which is the difference between this and
    /// [`render`](Self::render) below.
    pub fn words(&self) -> String {
        match self.msg_type.to_lowercase().as_str() {
            "text" => self.text.content.trim().to_string(),
            // WeCom runs the speech recognition on its side and delivers only
            // the result, so a voice run is a sentence that happened to be
            // spoken — no download, no key. It is the sender's own words, so
            // a spoken "/issue 登录坏了" is a command like the typed one.
            "voice" => self.voice.content.trim().to_string(),
            _ => String::new(),
        }
    }

    /// Turns one 图文混排 run into the line it contributes to the message
    /// body. An item of a kind this adapter does not know contributes nothing
    /// rather than a stray placeholder.
    pub fn render(&self) -> String {
        let s = self.words();
        if !s.is_empty() {
            return s;
        }
        let Some((body, kind)) = media_for(&self.msg_type, &self.image, &self.file, &self.video)
        else {
            return String::new();
        };
        if body.url.trim().is_empty() {
            return String::new();
        }
        media_placeholder(&kind).to_string()
    }
}

/// The marker that stands in for an attachment in the stored message body, so
/// the agent can see that something was attached before (or instead of) the
/// bytes arriving on the detached media path.
///
/// The exact strings are Lark's and DingTalk's, byte for byte. An agent reads
/// every channel through the same prompt; a wecom-only spelling would be one
/// more thing for it to learn for no reason.
pub fn media_placeholder(kind: &MsgType) -> &'static str {
    if *kind == MsgType::image() {
        "[Image]"
    } else if *kind == MsgType::video() {
        "[Video]"
    } else {
        "[File]"
    }
}

/// Returns the body and normalized kind for a raw wecom msgtype, or `None`
/// when that type is not one we download at all.
pub fn media_for(
    msg_type: &str,
    image: &MediaBody,
    file: &MediaBody,
    video: &MediaBody,
) -> Option<(MediaBody, MsgType)> {
    match msg_type.to_lowercase().as_str() {
        "image" => Some((image.clone(), MsgType::image())),
        "file" => Some((file.clone(), MsgType::file())),
        "video" => Some((video.clone(), MsgType::video())),
        _ => None,
    }
}

impl AibotMsgCallback {
    /// Lists the downloadable media on this callback, in the order the user
    /// sent it. A body with no url is skipped: there is nothing to fetch, and
    /// carrying it forward would only produce an intent-ledger row for an
    /// object that can never exist.
    pub fn attachments(&self) -> Vec<InboundMedia> {
        let mut out = Vec::new();
        let mut add = |body: &MediaBody, kind: MsgType| {
            if body.url.trim().is_empty() {
                return;
            }
            out.push(InboundMedia {
                kind: kind.0,
                url: body.url.clone(),
                aes_key: body.aes_key.clone(),
            });
        };
        if let Some((body, kind)) = media_for(&self.msg_type, &self.image, &self.file, &self.video)
        {
            add(&body, kind);
            return out;
        }
        if !self.msg_type.eq_ignore_ascii_case("mixed") {
            return out;
        }
        for item in &self.mixed.msg_item {
            if let Some((body, kind)) =
                media_for(&item.msg_type, &item.image, &item.file, &item.video)
            {
                add(&body, kind);
            }
        }
        out
    }

    /// The agent-readable body of this callback, and whether there is one at
    /// all.
    ///
    /// Plain text answers with its body; a voice note answers with the
    /// transcript WeCom recognised, which is the sender's own words and needs
    /// no download; a photo, file or video answers with a bracketed
    /// placeholder, because the bytes arrive later on a detached path and the
    /// message has to say something in the meantime (the placeholder is also
    /// what survives if the download never succeeds); 图文混排 answers with
    /// its runs rendered in the order they were composed, so "look at this"
    /// still reads above the picture it was written about.
    ///
    /// Recognition comes back empty on background noise or a half-second
    /// press, and an empty body would reach the agent as a turn with nothing
    /// in it — so an empty transcript answers `None` and takes the receipt
    /// path, exactly like a location card or a kind WeCom adds next year.
    pub fn own_text(&self) -> Option<String> {
        match self.msg_type.to_lowercase().as_str() {
            "text" => Some(self.text.content.clone()),
            "voice" => {
                let transcript = self.voice.content.trim().to_string();
                (!transcript.is_empty()).then_some(transcript)
            }
            "image" | "file" | "video" => {
                let (body, kind) = media_for(&self.msg_type, &self.image, &self.file, &self.video)?;
                if body.url.trim().is_empty() {
                    return None;
                }
                Some(media_placeholder(&kind).to_string())
            }
            "mixed" => {
                let runs: Vec<String> = self
                    .mixed
                    .msg_item
                    .iter()
                    .map(MixedItem::render)
                    .filter(|s| !s.is_empty())
                    .collect();
                if runs.is_empty() {
                    return None;
                }
                Some(runs.join("\n"))
            }
            _ => None,
        }
    }

    /// What the slash-command parsers read: the sender's own words, and
    /// nothing this adapter wrote.
    ///
    /// It is not [`own_text`](Self::own_text). own_text inserts "[Image]" /
    /// "[File]" / "[Video]" where the attachments were, because the stored
    /// body has to show that something was attached and where.
    /// `parse_issue_command` only ever looks at the FIRST non-empty line, so
    /// a person who attaches a screenshot and then types "/issue 登录坏了" —
    /// the natural order, and the one WeCom's composer encourages — produces
    /// a body opening with "[Image]", the parser sees a placeholder instead
    /// of the command, no issue is filed, and nothing anywhere tells them
    /// why. Typing the same two things in the other order works. That is not
    /// a distinction a user can be expected to know about.
    ///
    /// So the command source drops the placeholders and keeps the runs the
    /// sender authored, in order. The stored body still carries them: cutting
    /// them there would lose the position the detached media binder
    /// materializes into.
    ///
    /// A spoken message answers with its transcript. Those are the sender's
    /// own words as much as a typed line is, so "/issue 登录坏了" said out
    /// loud files the same issue it would have typed.
    ///
    /// A standalone photo, file or video answers with nothing. Its whole body
    /// is a placeholder, so there are no words in it to parse, and handing
    /// the parser a string the sender never typed is the defect this exists
    /// to remove.
    pub fn own_command_source(&self) -> String {
        match self.msg_type.to_lowercase().as_str() {
            "text" => self.text.content.clone(),
            "voice" => self.voice.content.trim().to_string(),
            "mixed" => self
                .mixed
                .msg_item
                .iter()
                .map(MixedItem::words)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }
}

/// The body of an aibot_event_callback frame. We only look at the event type;
/// specific event fields (template-card selection, feedback vote) are not
/// surfaced yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AibotEventCallback {
    #[serde(default)]
    pub event: EventBody,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventBody {
    #[serde(default, rename = "eventtype")]
    pub event_type: String,
}

// ---- normalization ----

/// The wecom-side flattened envelope the WS read loop builds from a decoded
/// aibot_msg_callback. It is stashed into
/// [`ChannelInboundMessage::raw`] as JSON so the wecom resolvers can reach
/// the platform-specific fields (BotID, ReqID) the cross-platform envelope
/// does not carry.
///
/// Port note: Go names this `InboundMessage`; Rust adds the Wecom prefix to
/// avoid clashing with [`patchbay_channel::InboundMessage`]. JSON tags are
/// unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WecomInboundMessage {
    /// The smart-bot identifier this event was delivered to. It is the
    /// routing key the installation resolver uses.
    #[serde(rename = "bot_id")]
    pub bot_id: String,

    /// The WeCom per-message identifier used for two-phase dedup.
    #[serde(default, rename = "msg_id", skip_serializing_if = "String::is_empty")]
    pub msg_id: String,

    /// The raw wecom type ("text", "image", "event", ...). Media / unknown
    /// types round-trip via the cross-platform [`MsgType`] enum; the raw
    /// string stays here for auditing.
    #[serde(default, rename = "msg_type", skip_serializing_if = "String::is_empty")]
    pub msg_type: String,

    /// The tencent-internal conversation discriminator ("single" for 1:1,
    /// "group" for a group chat).
    #[serde(
        default,
        rename = "chat_type",
        skip_serializing_if = "String::is_empty"
    )]
    pub chat_type: String,

    /// The userid (single) or chatid (group) that the message originated in —
    /// the routing identity for outbound + session binding.
    #[serde(default, rename = "chat_id", skip_serializing_if = "String::is_empty")]
    pub chat_id: String,

    /// The userid of the person who typed the message.
    #[serde(
        default,
        rename = "sender_user_id",
        skip_serializing_if = "String::is_empty"
    )]
    pub sender_user_id: String,

    /// The human-readable body: the user's words — typed, or as WeCom's
    /// recognition returned them for a voice note — and the placeholders
    /// standing in for their attachments. The cross-platform envelope's text
    /// field is populated from this.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,

    /// The frame req_id the server sent this message with. Kept so a future
    /// aibot_respond_msg (5s window) can echo it back; iteration 1 uses
    /// aibot_send_msg unconditionally and does not need it.
    #[serde(default, rename = "req_id", skip_serializing_if = "String::is_empty")]
    pub req_id: String,

    /// The attachments to fetch, in the order the user sent them. It is the
    /// MediaResolver's input and travels only in the envelope's raw field,
    /// which the engine passes along in memory and never persists — the urls
    /// lapse after five minutes and the keys are single-use, so neither
    /// belongs in a table or a log line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<InboundMedia>,
}

/// One downloadable attachment on a callback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundMedia {
    /// The normalized media type the attachment row is labelled with. Held as
    /// its string form because this struct only rides the in-memory raw
    /// envelope; consumers convert back via MsgType(newtype).
    pub kind: String,
    /// The pre-signed COS address, good for five minutes, needing no access
    /// token.
    pub url: String,
    /// Unlocks what comes back from [`InboundMedia::url`]. Long-connection
    /// mode mints one per url; see media_crypt.rs.
    #[serde(rename = "aeskey")]
    pub aes_key: String,
}

/// Decodes the wecom-side [`WecomInboundMessage`] from the cross-platform
/// envelope's raw field. Every resolver ends up doing this at least once; we
/// centralize the shape so a Raw change is a single edit.
///
/// Port of `wecomMsgFromRaw` (wecom_resolvers.go).
pub fn wecom_msg_from_raw(msg: &ChannelInboundMessage) -> anyhow::Result<WecomInboundMessage> {
    if msg.raw.is_null() {
        anyhow::bail!("wecom: inbound message Raw is empty");
    }
    serde_json::from_value(msg.raw.clone())
        .map_err(|e| anyhow::anyhow!("wecom: decode inbound raw: {e}"))
}

/// Converts a wecom-side aibot_msg_callback into the cross-platform
/// [`ChannelInboundMessage`] the engine Router consumes. The wecom-side
/// [`WecomInboundMessage`] is stashed in raw so the resolvers can access
/// platform-specific fields.
///
/// Routing identity:
/// - single → ChatType=p2p, ChatID=userid, SenderID=userid
/// - group  → ChatType=group, ChatID=chatid, SenderID=from.userid
///
/// A user @-mentioning the bot in a group is not distinguishable from a raw
/// group message on the wire — WeCom only forwards to the bot when it was
/// addressed, so any received group message counts as addressed.
///
/// `text` is the agent-readable body the caller already resolved via
/// [`AibotMsgCallback::own_text`]. It is passed in rather than recomputed
/// because the caller has to know whether the message is routable at all
/// before it gets here. The command source is a different string and is
/// derived here from mc — see [`AibotMsgCallback::own_command_source`] for
/// why they must not be the same one.
///
/// `bot_display_name` is the bot's name in a chat, from the installation
/// config. It is used for one thing: recognising where the sender's
/// @-mention ends. Empty is fine and falls back to a whitespace heuristic;
/// see [`strip_leading_mentions`].
pub fn channel_message_from_callback(
    bot_id: &str,
    bot_display_name: &str,
    mc: &AibotMsgCallback,
    text: &str,
    req_id: &str,
) -> ChannelInboundMessage {
    let chat_type = if mc.chat_type.eq_ignore_ascii_case("group") {
        ChatType::group()
    } else {
        ChatType::p2p()
    };
    let sender_id = mc.from.user_id.clone();
    let mut chat_id = mc.chat_id.clone();
    if chat_type == ChatType::p2p() && chat_id.is_empty() {
        // Some flavors set chat id only for groups; fall back to the sender.
        chat_id = sender_id.clone();
    }

    // The command source is the sender's own words — own_command_source, not
    // the resolved body, so a 图文混排 whose first run is a screenshot still
    // has its "/issue …" on the first line the parser reads.
    //
    // In a group the @-mention IS how you reach the bot, so it arrives glued
    // to whatever was typed after it — "@Andrew /new" is a person asking for
    // a fresh session, not prose that happens to contain a word — and the
    // addressing comes off the front.
    //
    // Groups only. In a 1:1 nobody has to address the bot, so a leading "@"
    // is the sender naming a colleague they are talking ABOUT: "@李雷 /issue
    // 帮我问问他" is a question, and stripping the name would turn it into a
    // filed issue nobody asked for plus, via SkipAgentRun below, no answer at
    // all.
    //
    // For a plain text message this is mc.text.content, which is what p2p was
    // passing through before CommandText was set here; for a voice note it is
    // the transcript, so a spoken command is a command.
    let mut command = mc.own_command_source();
    if chat_type == ChatType::group() {
        command = strip_leading_mentions(&command, bot_display_name);
    }

    let wm = WecomInboundMessage {
        bot_id: bot_id.to_string(),
        msg_id: mc.msg_id.clone(),
        msg_type: mc.msg_type.clone(),
        chat_type: mc.chat_type.clone(),
        chat_id: chat_id.clone(),
        sender_user_id: sender_id.clone(),
        content: text.to_string(),
        req_id: req_id.to_string(),
        media: mc.attachments(),
    };
    let raw = serde_json::to_value(&wm).unwrap_or(Value::Null);

    ChannelInboundMessage {
        event_id: mc.msg_id.clone(),
        message_id: mc.msg_id.clone(),
        r#type: channel_msg_type(&mc.msg_type),
        text: text.to_string(),
        addressed_to_bot: true,
        // The sender's own words, with a group's addressing removed and the
        // media placeholders left out. Command classification is shared and
        // falls back to Text when this is empty — and Text starts with the
        // mention in a group, and with "[Image]" whenever a screenshot came
        // first, so on that fallback every such slash command read as
        // ordinary prose.
        command_text: command.clone(),
        // A pure /issue command in WeCom should NOT trigger the agent — the
        // engine already creates the issue and the OutboundReplier already
        // sends "✅ 已创建 #N". Letting the agent see "/issue foo" then
        // produces a "I don't recognize this slash command" reply that just
        // clutters the conversation. wecom is alone on this — Slack/Lark keep
        // the historical "let the agent see /issue and respond too"
        // behaviour.
        //
        // Read off the same source the engine will parse, so a group /issue
        // behaves like the p2p one instead of filing the issue and then also
        // asking the agent about it. It has to be the same source: read off
        // the raw text instead and a p2p "@李雷 /issue …" would file an issue
        // and stay silent, which is the whole reason the strip above is
        // gated; read off the resolved body instead and a
        // screenshot-then-"/issue" message would skip the agent while the
        // parser declined the placeholder line, leaving the sender with
        // neither an issue nor an answer.
        skip_agent_run: is_issue_command(&command),
        source: Source {
            channel_type: type_wecom(),
            chat_id,
            chat_type,
            sender_id,
            ..Default::default()
        },
        raw,
        ..Default::default()
    }
}

/// Removes the @-mentions a message opens with, which in a group chat is how
/// the sender addresses the bot. WeCom puts them in the text and sends no
/// mention list alongside it, so there is nothing to match against but the
/// shape: an "@" at the very front, up to the next space.
///
/// Group messages only — the caller gates it on chat type. Nobody addresses
/// the bot in a 1:1, so the same "@" at the front there is a colleague's name
/// in the sender's own sentence, and removing it would rewrite what they
/// said.
///
/// Only the front. A name further into the sentence is the sender talking
/// ABOUT somebody — "@Andrew ask @李雷 about yesterday" is one instruction
/// naming one colleague — and stripping that would quietly rewrite what they
/// said.
///
/// This feeds command classification only. The stored message keeps the text
/// exactly as it arrived, so the transcript still shows who was addressed.
pub fn strip_leading_mentions(s: &str, bot_name: &str) -> String {
    let mut cur = s;
    loop {
        let trimmed = cur.trim_start_matches(char::is_whitespace);
        if !trimmed.starts_with('@') {
            return trimmed.to_string();
        }
        // Our own name first, matched whole. A display name may contain
        // spaces — "Patchbay Bot" is the obvious one — and cutting at the first
        // space would leave "Bot /new 重新分析", which is not a command, so
        // every slash command in that group would still be dropped.
        //
        // The name is not guessed. It comes from the installation config, set
        // when the bot was connected, because the callback carries no
        // structured mention list to read it from. Absent, the heuristic
        // below is what runs — correct for a one-word name, and what every
        // installation has until somebody fills the field in.
        let after_at = &trimmed[1..];
        if !bot_name.is_empty() && after_at.starts_with(bot_name) {
            cur = &after_at[bot_name.len()..];
            continue;
        }
        match trimmed.find(char::is_whitespace) {
            None => {
                // The whole message is one mention and nothing else. There is
                // no command and no words — leave it, so an empty body is
                // decided by the caller rather than manufactured here.
                return trimmed.to_string();
            }
            Some(i) => cur = &trimmed[i..],
        }
    }
}

/// Asks the engine's own parser instead of mirroring it. The mirror had
/// drifted: it trimmed with strings.TrimSpace, which strips every Unicode
/// space including U+3000 — the ideographic space a Chinese IME emits in
/// full-width mode — while parse_issue_command trims only " \t".
///
/// So a p2p line opening with U+3000 read as a command here and as prose
/// there: SkipAgentRun was set so no agent ran, and the parser declined so no
/// issue was filed. The sender got nothing back and no error anywhere said
/// why. Group messages reach this helper after their leading mentions are
/// normalized, and must use the same parser too.
///
/// A mirror of a parser is a parser. Delegating costs one allocation on a
/// path that already does I/O, and removes the whole class.
fn is_issue_command(body: &str) -> bool {
    patchbay_channel_engine::parse_issue_command(body).is_some()
}

/// Maps the raw aibot msg_type onto the normalized enum.
pub fn channel_msg_type(wecom_type: &str) -> MsgType {
    match wecom_type.to_lowercase().as_str() {
        "text" => MsgType::text(),
        "image" => MsgType::image(),
        "file" => MsgType::file(),
        "voice" | "audio" => MsgType::audio(),
        "video" => MsgType::video(),
        // 图文混排: text runs and attachments interleaved. It maps to Text
        // because the message IS text once own_text has rendered it — runs in
        // composition order, each attachment standing in as its placeholder —
        // and the attachments travel separately as MediaRefs, exactly as they
        // do for Lark's `post`, the same shape under another name.
        "mixed" => MsgType::text(),
        // A kind the adapter cannot read at all. dispatch_frame answers it
        // with the unsupported-kind receipt and stops, so this normalization
        // is never reached for one.
        _ => MsgType::unknown(),
    }
}

// ---- outbound helpers ----

/// Builds an aibot_subscribe body. The server responds with an echoed req_id
/// and errcode 0 on success.
pub fn subscribe_body(bot_id: &str, secret: &str) -> Value {
    serde_json::json!({ "bot_id": bot_id, "secret": secret })
}

/// Builds an aibot_send_msg body carrying plain-text content.
/// aibot_send_msg's supported msgtypes are markdown and template_card only —
/// text is NOT accepted on this cmd (contrast aibot_respond_msg, which does
/// accept text). We therefore ship as markdown; the WeCom client renders
/// plain text through the markdown path without any special escaping.
/// `chat_type` is 1 for single, 2 for group.
pub fn send_msg_text_body(chat_id: &str, chat_type: i64, content: &str) -> anyhow::Result<Value> {
    if chat_id.is_empty() {
        anyhow::bail!("wecom: send_msg requires chat_id");
    }
    if chat_type != CHAT_TYPE_SINGLE_INT && chat_type != CHAT_TYPE_GROUP_INT {
        anyhow::bail!("wecom: send_msg chat_type must be 1 (single) or 2 (group)");
    }
    Ok(serde_json::json!({
        "chatid": chat_id,
        "chat_type": chat_type,
        "msgtype": "markdown",
        "markdown": { "content": content },
    }))
}

/// Maps the engine's ChatType enum to the int the aibot_send_msg body wants.
pub fn aibot_chat_type_from_channel(t: &ChatType) -> i64 {
    if *t == ChatType::group() {
        CHAT_TYPE_GROUP_INT
    } else {
        CHAT_TYPE_SINGLE_INT
    }
}

/// Decodes the per-url AES key carried on a media body. Both the padded
/// 44-character form and the unpadded 43-character form appear in WeCom's own
/// surfaces, so both are accepted; anything that does not come out at exactly
/// 32 bytes is refused rather than stretched or truncated into one.
///
/// Port note: lives beside the frame types because the key arrives ON the
/// Convenience: base64-encodes bytes with the standard alphabet (Go's
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg_callback(json: Value) -> AibotMsgCallback {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn own_text_variants() {
        let text = msg_callback(json!({"msgtype":"text","text":{"content":" hi "}}));
        assert_eq!(text.own_text().as_deref(), Some(" hi "));

        let voice = msg_callback(json!({"msgtype":"voice","voice":{"content":"登录坏了"}}));
        assert_eq!(voice.own_text().as_deref(), Some("登录坏了"));

        let empty_voice = msg_callback(json!({"msgtype":"voice","voice":{"content":"  "}}));
        assert_eq!(empty_voice.own_text(), None);

        let image =
            msg_callback(json!({"msgtype":"image","image":{"url":"https://cos/x","aeskey":"k"}}));
        assert_eq!(image.own_text().as_deref(), Some("[Image]"));

        let image_no_url = msg_callback(json!({"msgtype":"image"}));
        assert_eq!(image_no_url.own_text(), None);

        let unknown = msg_callback(json!({"msgtype":"location"}));
        assert_eq!(unknown.own_text(), None);
    }

    #[test]
    fn own_text_mixed_renders_runs_in_order() {
        let mixed = msg_callback(json!({
            "msgtype": "mixed",
            "mixed": {"msg_item": [
                {"msgtype": "text", "text": {"content": "look at this"}},
                {"msgtype": "image", "image": {"url": "https://cos/a"}},
                {"msgtype": "voice", "voice": {"content": "please"}},
                {"msgtype": "file"},
            ]}
        }));
        assert_eq!(
            mixed.own_text().as_deref(),
            Some("look at this\n[Image]\nplease")
        );
    }

    #[test]
    fn own_command_source_drops_placeholders_keeps_words() {
        let mixed = msg_callback(json!({
            "msgtype": "mixed",
            "mixed": {"msg_item": [
                {"msgtype": "image", "image": {"url": "https://cos/a"}},
                {"msgtype": "text", "text": {"content": "/issue 登录坏了"}},
            ]}
        }));
        assert_eq!(mixed.own_command_source(), "/issue 登录坏了");

        let voice = msg_callback(json!({"msgtype":"voice","voice":{"content":"/issue 坏了"}}));
        assert_eq!(voice.own_command_source(), "/issue 坏了");
    }

    #[test]
    fn attachments_lists_media_in_sent_order_skips_urlless() {
        let mc = msg_callback(json!({
            "msgtype": "mixed",
            "mixed": {"msg_item": [
                {"msgtype": "image", "image": {"url": "https://cos/a", "aeskey": "k1"}},
                {"msgtype": "video"},
                {"msgtype": "file", "file": {"url": "https://cos/f", "aeskey": "k2"}},
            ]}
        }));
        let atts = mc.attachments();
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0].kind, MsgType::image().0);
        assert_eq!(atts[0].url, "https://cos/a");
        assert_eq!(atts[1].kind, MsgType::file().0);

        let plain =
            msg_callback(json!({"msgtype":"file","file":{"url":"https://cos/f","aeskey":"k"}}));
        assert_eq!(plain.attachments().len(), 1);

        let none = msg_callback(json!({"msgtype":"text"}));
        assert!(none.attachments().is_empty());
    }

    #[test]
    fn channel_message_routes_single_and_group() {
        let single = msg_callback(json!({
            "msgid": "m1", "chattype": "single",
            "from": {"userid": "u1"}, "msgtype": "text",
            "text": {"content": "hello"},
        }));
        let m = channel_message_from_callback("bot", "", &single, "hello", "r1");
        assert_eq!(m.source.chat_type, ChatType::p2p());
        assert_eq!(m.source.chat_id, "u1"); // falls back to the sender
        assert_eq!(m.source.sender_id, "u1");
        assert!(m.addressed_to_bot);
        assert!(!m.skip_agent_run);
        assert_eq!(m.raw["bot_id"], json!("bot"));
        assert_eq!(m.raw["req_id"], json!("r1"));

        let group = msg_callback(json!({
            "msgid": "m2", "chattype": "group", "chatid": "c1",
            "from": {"userid": "u2"}, "msgtype": "text",
            "text": {"content": "@Patchbay Bot /new 重新分析"},
        }));
        let m = channel_message_from_callback("bot", "Patchbay Bot", &group, "x", "");
        assert_eq!(m.source.chat_type, ChatType::group());
        assert_eq!(m.source.chat_id, "c1");
        assert_eq!(m.command_text, "/new 重新分析");
    }

    #[test]
    fn strip_leading_mentions_whole_name_first() {
        assert_eq!(
            strip_leading_mentions("@Patchbay Bot /new x", "Patchbay Bot"),
            "/new x"
        );
        // A heuristic cut KEEPS the separator space (Go slices from the
        // whitespace index), so one leading space survives this single pass.
        assert_eq!(strip_leading_mentions("@李雷 你好", ""), "你好");
        // Unknown mention: cut up to the next whitespace.
        assert_eq!(strip_leading_mentions("@李雷 你好", ""), "你好");
        // Whole-message mention stays put.
        assert_eq!(strip_leading_mentions("@李雷", ""), "@李雷");
        // No mention at all.
        assert_eq!(strip_leading_mentions("plain", "Bot"), "plain");
        // Only the front: mid-sentence mentions survive.
        assert_eq!(
            strip_leading_mentions("ask @李雷 about yesterday", ""),
            "ask @李雷 about yesterday"
        );
    }

    #[test]
    fn skip_agent_run_tracks_the_engine_parser() {
        let cmd = msg_callback(json!({
            "msgid": "m3", "chattype": "single",
            "from": {"userid": "u"}, "msgtype": "text",
            "text": {"content": "/issue 登录坏了"},
        }));
        let m = channel_message_from_callback("bot", "", &cmd, "/issue 登录坏了", "");
        assert!(m.skip_agent_run);

        let prose = msg_callback(json!({
            "msgid": "m4", "chattype": "single",
            "from": {"userid": "u"}, "msgtype": "text",
            "text": {"content": "hey /issue not a command"},
        }));
        let m = channel_message_from_callback("bot", "", &prose, "hey /issue not a command", "");
        assert!(!m.skip_agent_run);
    }

    #[test]
    fn channel_msg_type_mapping() {
        assert_eq!(channel_msg_type("Text"), MsgType::text());
        assert_eq!(channel_msg_type("IMAGE"), MsgType::image());
        assert_eq!(channel_msg_type("file"), MsgType::file());
        assert_eq!(channel_msg_type("voice"), MsgType::audio());
        assert_eq!(channel_msg_type("audio"), MsgType::audio());
        assert_eq!(channel_msg_type("video"), MsgType::video());
        assert_eq!(channel_msg_type("mixed"), MsgType::text());
        assert_eq!(channel_msg_type("location"), MsgType::unknown());
    }

    #[test]
    fn send_msg_body_validates_inputs() {
        assert!(send_msg_text_body("", 1, "x").is_err());
        assert!(send_msg_text_body("c", 3, "x").is_err());
        let v = send_msg_text_body("c", CHAT_TYPE_GROUP_INT, "hi").unwrap();
        assert_eq!(v["msgtype"], json!("markdown"));
        assert_eq!(v["markdown"]["content"], json!("hi"));
        assert_eq!(v["chat_type"], json!(CHAT_TYPE_GROUP_INT));
    }

    #[test]
    fn aibot_chat_type_maps_group_only() {
        assert_eq!(aibot_chat_type_from_channel(&ChatType::group()), 2);
        assert_eq!(aibot_chat_type_from_channel(&ChatType::p2p()), 1);
    }

    #[test]
    fn envelope_decodes_ack_fields() {
        let env: FrameEnvelope = serde_json::from_value(json!({
            "headers": {"req_id": "abc"},
            "errcode": 40001,
            "errmsg": "bad secret",
        }))
        .unwrap();
        assert_eq!(env.cmd, "");
        assert_eq!(env.headers.req_id, "abc");
        assert_eq!(env.err_code, 40001);
        assert_eq!(env.err_msg, "bad secret");
    }

    #[test]
    fn wecom_msg_roundtrips_through_raw() {
        let mc = msg_callback(json!({
            "msgid": "m5", "chattype": "group", "chatid": "c",
            "from": {"userid": "u"}, "msgtype": "image",
            "image": {"url": "https://cos/i", "aeskey": "kk"},
        }));
        let m = channel_message_from_callback("bot", "", &mc, "[Image]", "rq");
        let back = wecom_msg_from_raw(&m).unwrap();
        assert_eq!(back.bot_id, "bot");
        assert_eq!(back.msg_id, "m5");
        assert_eq!(back.media.len(), 1);
        assert_eq!(back.media[0].aes_key, "kk");
        assert_eq!(back.req_id, "rq");
    }

    #[test]
    fn decode_aes_key_delegates_to_media_crypt() {
        use base64::Engine as _;
        let key = [7u8; 32];
        let enc = base64::engine::general_purpose::STANDARD.encode(key);
        assert_eq!(crate::media_crypt::decode_media_aes_key(&enc).unwrap(), key);
    }
}
