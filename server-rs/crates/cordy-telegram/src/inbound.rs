//! Inbound normalization: Telegram Update → the engine's normalized
//! channel::InboundMessage.
//!
//! Port of `server/internal/integrations/telegram/inbound.go`.

use serde::{Deserialize, Serialize};

use cordy_channel::{ChatType, InboundMessage, MsgType, ReplyCtx, Source};
use cordy_channel_engine::parse_fresh_session_command;

#[cfg(test)]
use crate::api::TelegramChat;
use crate::api::{MessageEntity, TelegramMessage, TelegramUser, Update};

/// The platform-specific raw event stashed on InboundMessage::raw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramRawEvent {
    #[serde(rename = "bot_id")]
    pub bot_id: String,
    #[serde(rename = "event_type")]
    pub event_type: String,
    #[serde(
        rename = "sender_name",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub sender_name: String,
}

/// Translates one poll update into the normalized envelope; `false` when
/// the update is not an addressable user message (non-message updates,
/// bot-authored messages, unsupported chat types).
pub fn inbound_from_update(u: &Update, bot_id: i64, bot_username: &str) -> Option<InboundMessage> {
    let m = u.message.as_ref()?;
    let from = m.from.as_ref()?;
    if from.is_bot || from.id == bot_id {
        return None;
    }
    let chat_type = telegram_chat_type(&m.chat.r#type)?;

    let mut text = m.text.clone();
    if text.is_empty() {
        text = m.caption.clone();
    }
    let msg_type = classify_message(m);

    let mentioned = mentions_bot(m, bot_username);
    let replied_to_bot = m
        .reply_to_message
        .as_ref()
        .and_then(|r| r.from.as_ref())
        .is_some_and(|f| f.id == bot_id);
    let addressed = chat_type == ChatType::p2p() || mentioned || replied_to_bot;

    let cleaned = normalize_text(&text, bot_username);
    let command_text = cleaned.clone();
    let mut force_fresh = false;
    let cleaned = match parse_fresh_session_command(&cleaned) {
        Some(body) => {
            force_fresh = true;
            body
        }
        None => cleaned,
    };
    let mut agent_text = cleaned.clone();
    let quoted_human = m
        .reply_to_message
        .as_ref()
        .and_then(|r| r.from.as_ref())
        .is_some_and(|f| !f.is_bot);
    if chat_type == ChatType::group() && mentioned && quoted_human {
        agent_text = enrich_with_quoted_human_message(
            &cleaned,
            m.chat.id,
            m.reply_to_message.as_deref().unwrap(),
        );
    }

    let sender_id = from.id.to_string();
    let chat_id = m.chat.id.to_string();
    let mut thread_id = String::new();
    if m.is_topic_message && m.message_thread_id != 0 {
        thread_id = m.message_thread_id.to_string();
    }

    let raw = serde_json::to_value(TelegramRawEvent {
        bot_id: bot_id.to_string(),
        event_type: "message".to_string(),
        sender_name: sender_display_name(from),
    })
    .unwrap_or(serde_json::Value::Null);

    let reply = m.reply_to_message.as_ref().map(|r| ReplyCtx {
        message_id: message_key(m.chat.id, r.message_id),
        root_id: thread_id.clone(),
    });

    Some(InboundMessage {
        event_id: u.update_id.to_string(),
        message_id: message_key(m.chat.id, m.message_id),
        r#type: msg_type,
        text: agent_text,
        command_text,
        reply_to: reply,
        addressed_to_bot: addressed,
        force_fresh,
        source: Source {
            channel_type: cordy_channel::Type(crate::TYPE_TELEGRAM.to_string()),
            chat_id,
            chat_type,
            sender_id: sender_id.clone(),
            sender_stable_id: sender_id,
            thread_id,
        },
        raw,
        ..Default::default()
    })
}

/// The platform message reference: "<chat_id>:<message_id>".
pub fn message_key(chat_id: i64, message_id: i64) -> String {
    format!("{chat_id}:{message_id}")
}

/// Maps a Telegram chat type onto the normalized set; `None` for
/// channels and anything else the engine does not model.
pub fn telegram_chat_type(t: &str) -> Option<ChatType> {
    match t {
        "private" => Some(ChatType::p2p()),
        "group" | "supergroup" => Some(ChatType::group()),
        _ => None,
    }
}

fn classify_message(m: &TelegramMessage) -> MsgType {
    if !m.text.is_empty() {
        return MsgType::text();
    }
    if !m.photo.is_empty() {
        return MsgType::image();
    }
    if m.voice.is_some() {
        return MsgType::audio();
    }
    if m.video.is_some() {
        return MsgType::video();
    }
    if m.document.is_some() {
        return MsgType::file();
    }
    MsgType::unknown()
}

fn mentions_bot(m: &TelegramMessage, bot_username: &str) -> bool {
    if bot_username.is_empty() {
        return false;
    }
    let want_mention = format!("@{bot_username}");
    for (text, entities) in [
        (m.text.as_str(), &m.entities),
        (m.caption.as_str(), &m.caption_entities),
    ] {
        for entity in entities {
            if entity.r#type != "mention" && entity.r#type != "bot_command" {
                continue;
            }
            let Some(value) = message_entity_text(text, entity) else {
                continue;
            };
            if value.eq_ignore_ascii_case(&want_mention)
                || command_targets_bot(&value, bot_username)
            {
                return true;
            }
        }
    }
    contains_bot_mention(&m.text, bot_username) || contains_bot_mention(&m.caption, bot_username)
}

/// Strips @bot mentions and trims. The user's own words remain.
pub fn normalize_text(text: &str, bot_username: &str) -> String {
    let cleaned = if bot_username.is_empty() {
        text.to_string()
    } else {
        remove_bot_mentions(text, bot_username)
    };
    cleaned.trim().to_string()
}

/// Builds the quoted-context block appended when a user replies to another
/// human's message while mentioning the bot in a group.
fn enrich_with_quoted_human_message(
    instruction: &str,
    _chat_id: i64,
    quoted: &TelegramMessage,
) -> String {
    let mut quoted_text = quoted.text.clone();
    if quoted_text.is_empty() {
        quoted_text = quoted.caption.clone();
    }
    if quoted_text.trim().is_empty() {
        quoted_text = "[empty or non-text message]".to_string();
    }
    let sender = quoted
        .from
        .as_ref()
        .map(sender_display_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Unknown user".to_string());
    // The Go format uses %q for the id/sender/type fields; ids are plain
    // integers so quoting only matters for strings.
    let block = format!(
        "<quoted_message message_id=\"{}\" sender=\"{}\" type=\"{}\">\n{}\n</quoted_message>",
        crate::message_key(0, quoted.message_id),
        go_quote(&sender),
        go_quote(&MsgType::unknown().0),
        quoted_text
    );
    if instruction.is_empty() {
        return block;
    }
    format!("{block}\n\n{instruction}")
}

/// strconv.Quote-equivalent double-quoting for the ASCII subset the
/// quoted-context block carries.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn command_targets_bot(command: &str, bot_username: &str) -> bool {
    match command.rfind('@') {
        Some(at) => command[at + 1..].eq_ignore_ascii_case(bot_username),
        None => false,
    }
}

/// Extracts the entity substring honoring Telegram's UTF-16 code-unit
/// offsets.
fn message_entity_text(text: &str, entity: &MessageEntity) -> Option<String> {
    if entity.length == 0 || usize::checked_add(entity.offset, entity.length).is_none() {
        return None;
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let end = entity.offset + entity.length;
    if entity.offset > units.len() || end > units.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&units[entity.offset..end]))
}

fn contains_bot_mention(text: &str, bot_username: &str) -> bool {
    let token = format!("@{}", bot_username.to_lowercase());
    if token.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    let lower_bytes = lower.as_bytes();
    let token_bytes = token.as_bytes();
    let mut start = 0;
    loop {
        let Some(i) = lower[start..].find(&token) else {
            return false;
        };
        let i = start + i;
        let end = i + token_bytes.len();
        if end == lower.len() || !is_telegram_username_byte(lower_bytes[end]) {
            return true;
        }
        start = end;
    }
}

fn remove_bot_mentions(text: &str, bot_username: &str) -> String {
    let token = format!("@{}", bot_username.to_lowercase());
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut start = 0;
    while start < text.len() {
        let Some(i_rel) = lower[start..].find(&token) else {
            out.push_str(&text[start..]);
            break;
        };
        let i = start + i_rel;
        let end = i + token.len();
        if end < lower.len() && is_telegram_username_byte(lower.as_bytes()[end]) {
            // Longer username continuing after @bot — keep scanning past it.
            out.push_str(&text[start..end]);
            start = end;
            continue;
        }
        out.push_str(&text[start..i]);
        start = end;
    }
    out
}

fn is_telegram_username_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_lowercase() || b.is_ascii_digit()
}

fn sender_display_name(u: &TelegramUser) -> String {
    let mut name = u.first_name.clone();
    if !u.last_name.is_empty() {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(&u.last_name);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64, is_bot: bool) -> TelegramUser {
        TelegramUser {
            id,
            is_bot,
            first_name: "Ann".into(),
            ..Default::default()
        }
    }

    fn msg(chat_type: &str, text: &str) -> TelegramMessage {
        TelegramMessage {
            message_id: 10,
            from: Some(user(42, false)),
            chat: TelegramChat {
                id: -100,
                r#type: chat_type.into(),
            },
            text: text.into(),
            ..Default::default()
        }
    }

    const BOT_ID: i64 = 7;

    #[test]
    fn skips_non_user_and_bot_messages() {
        assert!(inbound_from_update(&Update::default(), BOT_ID, "").is_none());
        // Bot author.
        let mut u = Update {
            update_id: 1,
            message: Some(msg("private", "hi")),
        };
        u.message.as_mut().unwrap().from = Some(user(BOT_ID, true));
        assert!(inbound_from_update(&u, BOT_ID, "").is_none());
        // Channel posts are not modeled.
        let mut u2 = Update {
            update_id: 2,
            message: Some(msg("channel", "hi")),
        };
        u2.message.as_mut().unwrap().from = Some(user(42, false));
        assert!(inbound_from_update(&u2, BOT_ID, "").is_none());
    }

    #[test]
    fn p2p_message_is_addressed_and_normalized() {
        let u = Update {
            update_id: 5,
            message: Some(msg("private", "  hello  ")),
        };
        let m = inbound_from_update(&u, BOT_ID, "mybot").unwrap();
        assert_eq!(m.event_id, "5");
        assert_eq!(m.text, "hello");
        assert_eq!(m.command_text, "hello");
        assert!(m.addressed_to_bot);
        assert_eq!(m.source.chat_id, "-100");
        assert_eq!(m.source.sender_id, "42");
        assert_eq!(m.source.channel_type.0, crate::TYPE_TELEGRAM);
        assert_eq!(m.r#type, MsgType::text());
    }

    #[test]
    fn group_requires_addressing() {
        let u = Update {
            update_id: 6,
            message: Some(msg("supergroup", "plain chatter")),
        };
        let m = inbound_from_update(&u, BOT_ID, "mybot").unwrap();
        assert!(!m.addressed_to_bot);

        let mention_text = "hey @MyBot do a thing";
        let mut um = msg("supergroup", mention_text);
        um.entities = vec![MessageEntity {
            r#type: "mention".into(),
            offset: 4,
            length: "@mybot".encode_utf16().count(),
        }];
        let mu = Update {
            update_id: 7,
            message: Some(um),
        };
        let m = inbound_from_update(&mu, BOT_ID, "mybot").unwrap();
        assert!(m.addressed_to_bot);
        // Only the token is removed; the inner space remains (Go parity).
        assert_eq!(m.text, "hey  do a thing");
    }

    #[test]
    fn fresh_command_sets_flag_and_rewrites_text() {
        // /new is the shared fresh-session command parsed by the engine.
        let u = Update {
            update_id: 8,
            message: Some(msg("private", "/new")),
        };
        let m = inbound_from_update(&u, BOT_ID, "").unwrap();
        assert!(m.force_fresh);
        assert_eq!(m.command_text, "/new");

        // Non-command messages keep the flag off.
        let u = Update {
            update_id: 9,
            message: Some(msg("private", "just chatting")),
        };
        let m = inbound_from_update(&u, BOT_ID, "").unwrap();
        assert!(!m.force_fresh);
    }

    #[test]
    fn media_classification_table() {
        let mut m = msg("private", "");
        m.photo = vec![serde_json::json!({})];
        assert_eq!(classify_message(&m), MsgType::image());
        m.photo.clear();
        m.voice = Some(serde_json::json!({}));
        assert_eq!(classify_message(&m), MsgType::audio());
        m.voice = None;
        m.video = Some(serde_json::json!({}));
        assert_eq!(classify_message(&m), MsgType::video());
        m.video = None;
        m.document = Some(crate::api::DocumentRef::default());
        assert_eq!(classify_message(&m), MsgType::file());
        m.document = None;
        assert_eq!(classify_message(&m), MsgType::unknown());

        // Caption-carrying media still classifies by attachment but keeps
        // caption as text fallback.
        m.document = Some(crate::api::DocumentRef {
            file_name: "f.pdf".into(),
        });
        m.caption = "see attached".into();
        let classified = inbound_from_update(
            &Update {
                update_id: 1,
                message: Some(m.clone()),
            },
            BOT_ID,
            "",
        )
        .unwrap();
        assert_eq!(classified.r#type, MsgType::file());
        assert_eq!(classified.text, "see attached");
    }

    #[test]
    fn utf16_entity_offsets_slice_correctly() {
        // Emoji are surrogate pairs — Telegram offsets count UTF-16 units.
        let text = "🎉 @mybot";
        let e = MessageEntity {
            r#type: "mention".into(),
            offset: 3, // emoji (2 units) + space
            length: "@mybot".encode_utf16().count(),
        };
        assert_eq!(message_entity_text(text, &e).as_deref(), Some("@mybot"));
        assert!(message_entity_text(
            "short",
            &MessageEntity {
                r#type: "mention".into(),
                offset: 50,
                length: 3,
            }
        )
        .is_none());
    }

    #[test]
    fn mention_detection_boundary_cases() {
        // Substring of a longer username does not count.
        assert!(!contains_bot_mention("@mybotfan hello", "mybot"));
        assert!(
            contains_bot_mention("@MYBOT hi", "mybot"),
            "case-insensitive"
        );
        assert!(contains_bot_mention("prefix@mybot!", "mybot"));
        assert!(!contains_bot_mention("no mention here", "mybot"));

        let removed = remove_bot_mentions("@mybot please @mybotfan stay", "mybot");
        assert_eq!(removed, " please @mybotfan stay");
    }

    #[test]
    fn quoted_human_enrichment_block_shape() {
        let mut quoted = msg("supergroup", "earlier human words");
        quoted.from = Some(user(99, false));
        let enriched = enrich_with_quoted_human_message("fix this", -100, &quoted);
        assert!(enriched.starts_with("<quoted_message "));
        assert!(enriched.contains("earlier human words"));
        assert!(enriched.ends_with("\n\nfix this"));

        // Empty instruction returns just the block.
        let block_only = enrich_with_quoted_human_message("", -100, &quoted);
        assert!(block_only.ends_with("</quoted_message>"));
    }
}
