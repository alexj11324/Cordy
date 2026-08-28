//! JSON event decoder.
//!
//! LarkJSONFrameDecoder decodes the JSON event payload Lark nests inside a
//! long-conn data Frame. The outer binary Frame envelope ([`crate::ws_frame`])
//! is stripped by the connector; the decoder only sees the bytes from
//! Frame.payload, which Lark formats as the standard event-subscription
//! envelope: {schema, header, event}.
//!
//! Three outcomes:
//!
//! - `Ok(Some(msg))` — `im.message.receive_v1` event. The Hub forwards
//!   through the Dispatcher.
//! - `Ok(None)`      — heartbeat-shaped JSON or an event_type we don't yet
//!   handle (im.chat.access_event_v1, etc.). The connector drops these
//!   silently and still sends a 200 ACK to Lark so the server stops resending.
//! - `Err(_)`        — malformed JSON or schema we couldn't parse. The
//!   connector logs + drops the single frame; the WS connection stays up
//!   because one bad payload shouldn't amplify into a reconnect storm.
//!
//! The decoder is stateless and thread-safe — a single instance serves every
//! supervisor task.

use serde::Deserialize;

use crate::content_flatten::flatten_content;
use crate::feishu_types::InboundMessage;
use crate::store::Installation;
use crate::types::{chat_type_group, chat_type_p2p, ChatId, ChatType, OpenId};
use crate::ws_connector::FrameDecoder;

/// LarkJSONFrameDecoder decodes the JSON payload of a data Frame.
pub struct LarkJsonFrameDecoder;

impl LarkJsonFrameDecoder {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LarkJsonFrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder for LarkJsonFrameDecoder {
    /// Implements FrameDecoder.
    #[allow(clippy::too_many_lines)]
    fn decode(
        &self,
        payload: &[u8],
        inst: &Installation,
    ) -> anyhow::Result<Option<InboundMessage>> {
        if payload.is_empty() {
            return Ok(None);
        }
        let env: LarkEventEnvelope =
            serde_json::from_slice(payload).map_err(|e| anyhow::anyhow!("envelope: {e}"))?;

        // Lark long-conn data frames are always v2 event envelopes (schema
        // "2.0"). The legacy webhook v1 "type":"event_callback" shape is not
        // used on long-conn — we accept it defensively in case Lark adds a
        // back-compat mode, but the canonical path is schema-driven.
        if !env.r#type.is_empty() && env.r#type != "event_callback" {
            return Ok(None);
        }

        if env.header.event_type != "im.message.receive_v1" {
            return Ok(None);
        }

        let Some(event) = env.event else {
            anyhow::bail!("event_callback with empty event payload");
        };
        let evt: LarkMessageReceiveEvent =
            serde_json::from_value(event).map_err(|e| anyhow::anyhow!("event: {e}"))?;

        let bot_union_id = inst.bot_union_id.clone().unwrap_or_default();

        let mut msg = InboundMessage {
            event_type: env.header.event_type.clone(),
            event_id: env.header.event_id.clone(),
            app_id: env.header.app_id.clone(),
            chat_id: ChatId(evt.message.chat_id.clone()),
            chat_type: normalize_chat_type(&evt.message.chat_type),
            message_id: evt.message.message_id.clone(),
            sender_open_id: OpenId(evt.sender.sender_id.open_id.clone()),
            message_type: evt.message.message_type.clone(),
            content: evt.message.content.clone(),
            create_time: evt.message.create_time.clone(),
            // parent_id / root_id are populated by Lark only in reply
            // scenarios. The enricher keys quoted-reply expansion off
            // parent_id (the directly quoted message); root_id is carried for
            // completeness / future thread handling.
            parent_id: evt.message.parent_id.clone(),
            root_id: evt.message.root_id.clone(),
            // thread_id is present only when the message lives inside a Lark
            // topic (话题). The outbound patcher uses it to decide whether to
            // reply back into that thread; empty means a normal chat message.
            thread_id: evt.message.thread_id.clone(),
            ..InboundMessage::default()
        };

        // text + post are flattened synchronously here (no external calls —
        // the decoder must stay fast and dependency-free). merge_forward
        // leaves body empty: it needs an HTTP round-trip to expand and is
        // handled downstream by the enricher, which keys off message_type.
        // Standalone media gets a short visible marker while the channel
        // adapter separately downloads and binds the binary as a Patchbay
        // attachment.
        match evt.message.message_type.as_str() {
            "text" | "post" => {
                msg.body = resolve_mentions(
                    &flatten_content(&evt.message.message_type, &evt.message.content),
                    &evt.message.mentions,
                    &inst.bot_open_id,
                    &bot_union_id,
                );
            }
            "image" | "file" | "audio" | "media" | "video" => {
                msg.body = flatten_content(&evt.message.message_type, &evt.message.content);
            }
            _ => {}
        }

        // Snapshot the user's own text as the command source BEFORE any
        // enrichment runs. The enricher rewrites body (prepending quoted /
        // forwarded context) but never touches command_body, so `/issue …`
        // is still parsed against what the user actually typed.
        msg.command_body = msg.body.clone();

        if msg.chat_type == chat_type_group() {
            msg.addressed_to_bot =
                contains_mention(&evt.message.mentions, &inst.bot_open_id, &bot_union_id);
        }

        Ok(Some(msg))
    }
}

/// Mirrors the outer JSON Lark wraps every push in.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LarkEventEnvelope {
    schema: String,
    r#type: String,
    header: LarkEventHeader,
    event: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LarkEventHeader {
    event_id: String,
    event_type: String,
    create_time: String,
    app_id: String,
    tenant_key: String,
}

/// larkMessageReceiveEvent is the documented payload of
/// im.message.receive_v1.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LarkMessageReceiveEvent {
    sender: LarkSender,
    message: LarkMessagePayload,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LarkSender {
    sender_id: LarkSenderId,
    sender_type: String,
    tenant_key: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LarkSenderId {
    open_id: String,
    union_id: String,
    user_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LarkMessagePayload {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    content: String,
    mentions: Vec<LarkMention>,
    create_time: String,
    /// Only present when the message is a reply / quote. parent_id is the
    /// directly quoted message; root_id is the root of the reply tree.
    parent_id: String,
    root_id: String,
    /// Present only for messages inside a Lark topic (话题). Lark omits it
    /// for plain chat messages, so its presence is the signal that an
    /// @-mention happened inside a thread.
    thread_id: String,
}

/// The WS-event mention shape: `id` is a nested {open_id, union_id, user_id}
/// object (unlike the IM REST item shape where id is a bare string).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LarkMention {
    pub key: String,
    pub id: LarkMentionId,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LarkMentionId {
    pub open_id: String,
    pub union_id: String,
    pub user_id: String,
}

/// resolve_mentions substitutes Lark's `@_user_N` placeholders so the agent
/// receives a body that reads naturally and does not require resolving the
/// mentions array itself. The bot's OWN mention is stripped (the dispatcher
/// already routes the event on addressed_to_bot — re-emitting `@<bot>` in
/// front of every message makes both the chat transcript and any downstream
/// LLM context noisier without adding signal). Other participants render as
/// `@<displayName>`, falling back to leaving the placeholder alone when name
/// is empty (defensive — Lark always populates it in practice).
///
/// Replacement is a single-pass token scan, not naive ReplaceAll. Two reasons:
///
/// - Prefix collision: a chat with eleven @-mentions exposes keys `@_user_1`
///   and `@_user_10`; ReplaceAll for `@_user_1` would mangle the substring of
///   `@_user_10`. We sort keys by length DESC and try the longest match at
///   each scan position so the longer placeholder always wins.
///
/// - Whitespace fidelity: when we strip the bot mention we only touch a
///   single space immediately adjacent to it — either the space after the
///   placeholder, or, if there is none, a single trailing space already in
///   the output. Tabs, indentation, code blocks, table pipes, and any other
///   intentional whitespace in the user's message are preserved verbatim.
///
/// Port note: the scan is byte-oriented exactly like Go's — UTF-8 continuation
/// bytes never collide with the ASCII placeholder keys, so rune boundaries are
/// preserved by construction.
pub fn resolve_mentions(
    text: &str,
    mentions: &[LarkMention],
    bot_open_id: &str,
    bot_union_id: &str,
) -> String {
    if text.is_empty() || mentions.is_empty() {
        return text.to_string();
    }
    // Filter empty keys and sort longest first so `@_user_10` is matched
    // before `@_user_1` at any scan position.
    let mut sorted: Vec<&LarkMention> = mentions.iter().filter(|m| !m.key.is_empty()).collect();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.key.len()));

    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let matched = sorted
            .iter()
            .find(|m| bytes[i..].starts_with(m.key.as_bytes()));
        let Some(m) = matched else {
            out.push(bytes[i]);
            i += 1;
            continue;
        };
        let mut end = i + m.key.len();
        if is_bot_mention(m, bot_open_id, bot_union_id) {
            // Strip: eat one adjacent space (after the placeholder preferred;
            // else backtrack one space we already emitted) so the seam is not
            // left with a double space or a dangling leading space. Tabs /
            // newlines / other chars are untouched.
            if end < bytes.len() && bytes[end] == b' ' {
                end += 1;
            } else if out.last() == Some(&b' ') {
                out.pop();
            }
        } else if !m.name.is_empty() {
            out.push(b'@');
            out.extend_from_slice(m.name.as_bytes());
        } else {
            // Unknown mention — leave the placeholder intact so the agent at
            // least sees a stable token.
            out.extend_from_slice(m.key.as_bytes());
        }
        i = end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// is_bot_mention identifies whether a payload mention refers to THIS bot.
/// Stays in lockstep with contains_mention: when union_id is known we trust
/// it exclusively (open_id is structurally inverted in multi-bot groups —
/// matching on it would re-introduce the PB-2671 routing bug). Only when
/// union_id is missing do we fall back to open_id, which is correct in
/// single-bot installs and the best we can do in pre-backfill rows.
fn is_bot_mention(m: &LarkMention, bot_open_id: &str, bot_union_id: &str) -> bool {
    if !bot_union_id.is_empty() {
        return m.id.union_id == bot_union_id;
    }
    if bot_open_id.is_empty() {
        return false;
    }
    m.id.open_id == bot_open_id
}

pub(crate) fn normalize_chat_type(t: &str) -> ChatType {
    match t.to_lowercase().as_str() {
        "p2p" => chat_type_p2p(),
        "group" => chat_type_group(),
        _ => patchbay_channel::message::ChatType(t.to_string()),
    }
}

/// contains_mention answers "was THIS bot @-mentioned in this group event".
///
/// The bot's stable identifier across WS perspectives is `union_id` — see
/// PB-2671 group-@-mention triage. In a Lark group with several Patchbay bots,
/// each bot's WS receives the event, and Lark fills
/// `mentions[].id.open_id` with the per-app form for whichever bot it is
/// talking to: bot X's WS sees X's payload-form open_id when bot Y was @-ed,
/// and a different payload-form open_id when X itself was the target. Only
/// `union_id` is consistent across both WS streams.
///
/// Match order:
///
/// 1. When we know the bot's `union_id` (captured by get_bot_info at install
///    time, persisted on the installation config), compare against
///    `mentions[].id.union_id`. This is the correct path and is unambiguous
///    in multi-bot deployments.
/// 2. When `union_id` is unknown — single-bot installs created before the
///    backfill migration, or contact-scope-restricted operators where
///    /contact/v3/users denied the lookup — fall back to the per-app
///    `open_id` comparison. This is structurally inverted in multi-bot group
///    chats but is fine for the p2p/single-bot case the WS sees most of the
///    time, and avoids hard-failing pre-backfill installations.
///
/// Empty inputs short-circuit to false rather than matching every mention;
/// that defends against an installation row that somehow has both identifiers
/// blank.
pub fn contains_mention(mentions: &[LarkMention], bot_open_id: &str, bot_union_id: &str) -> bool {
    if !bot_union_id.is_empty() {
        return mentions.iter().any(|m| m.id.union_id == bot_union_id);
    }
    if bot_open_id.is_empty() {
        return false;
    }
    mentions.iter().any(|m| m.id.open_id == bot_open_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_flatten::extract_text_body;
    use crate::store::Installation;
    use uuid::Uuid;

    fn installation(bot_open_id: &str, bot_union_id: Option<&str>) -> Installation {
        Installation {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            app_id: "cli_a".into(),
            app_secret_encrypted: Vec::new(),
            tenant_key: None,
            bot_open_id: bot_open_id.into(),
            installer_user_id: Uuid::nil(),
            status: "active".into(),
            ws_lease_token: None,
            ws_lease_expires_at: None,
            installed_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            bot_union_id: bot_union_id.map(str::to_string),
            region: "feishu".into(),
        }
    }

    fn decode_json(
        json: serde_json::Value,
        inst: &Installation,
    ) -> anyhow::Result<Option<InboundMessage>> {
        let d = LarkJsonFrameDecoder::new();
        d.decode(json.to_string().as_bytes(), inst)
    }

    fn text_event(
        chat_type: &str,
        content: &str,
        mentions: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_id": "evt_1",
                "event_type": "im.message.receive_v1",
                "create_time": "1700000000000",
                "app_id": "cli_a",
                "tenant_key": "tk"
            },
            "event": {
                "sender": {"sender_id": {"open_id": "ou_sender"}, "sender_type": "user"},
                "message": {
                    "message_id": "om_1",
                    "chat_id": "oc_1",
                    "chat_type": chat_type,
                    "message_type": "text",
                    "content": content,
                    "mentions": mentions,
                    "create_time": "1700000000000"
                }
            }
        })
    }

    #[test]
    fn decodes_text_message_and_strips_bot_mention() {
        let inst = installation("ou_bot", Some("un_bot"));
        let msg = decode_json(
            text_event(
                "group",
                r#"{"text":"@_user_1 你好 @__ALL"}"#,
                serde_json::json!([
                    {"key": "@_user_1", "id": {"open_id": "ou_bot", "union_id": "un_bot"}, "name": "Patchbay"},
                    {"key": "@__ALL", "id": {"open_id": "ou_all"}, "name": "所有人"}
                ]),
            ),
            &inst,
        )
        .unwrap()
        .expect("receive_v1 yields a message");
        assert_eq!(msg.event_id, "evt_1");
        assert_eq!(msg.app_id, "cli_a");
        assert_eq!(msg.chat_id.0, "oc_1");
        assert_eq!(msg.chat_type, chat_type_group());
        assert_eq!(msg.sender_open_id.0, "ou_sender");
        assert_eq!(msg.body, "你好 @所有人");
        assert_eq!(msg.command_body, "你好 @所有人");
        assert!(msg.addressed_to_bot);
    }

    #[test]
    fn union_id_takes_priority_over_inverted_open_id() {
        // Multi-bot group: the payload-form open_id on the mention does NOT
        // equal our stored bot_open_id; only union_id matches.
        let inst = installation("ou_mine", Some("un_mine"));
        let msg = decode_json(
            text_event(
                "group",
                r#"{"text":"hi @_user_1"}"#,
                serde_json::json!([
                    {"key": "@_user_1", "id": {"open_id": "ou_payload_form", "union_id": "un_mine"}, "name": "Bot"}
                ]),
            ),
            &inst,
        )
        .unwrap()
        .unwrap();
        assert!(msg.addressed_to_bot);
        assert_eq!(msg.body, "hi");
    }

    #[test]
    fn p2p_messages_are_always_addressed() {
        let inst = installation("ou_bot", None);
        let msg = decode_json(
            text_event("p2p", r#"{"text":"hello"}"#, serde_json::json!([])),
            &inst,
        )
        .unwrap()
        .unwrap();
        assert_eq!(msg.chat_type, chat_type_p2p());
        assert!(!msg.addressed_to_bot); // flag is group-only
        assert_eq!(msg.body, "hello");
    }

    #[test]
    fn media_types_get_placeholder_bodies() {
        let inst = installation("ou_bot", None);
        let mut evt = text_event("p2p", r#"{"image_key":"img_1"}"#, serde_json::json!([]));
        evt["event"]["message"]["message_type"] = serde_json::json!("image");
        let msg = decode_json(evt, &inst).unwrap().unwrap();
        assert_eq!(msg.body, "[Image]");
        assert_eq!(msg.command_body, "[Image]");
    }

    #[test]
    fn non_receive_events_are_silently_dropped() {
        let inst = installation("ou_bot", None);
        let heartbeat = serde_json::json!({"type": "heartbeat", "payload": {}});
        assert!(decode_json(heartbeat, &inst).unwrap().is_none());

        let other_event = serde_json::json!({
            "schema": "2.0",
            "header": {"event_id": "e", "event_type": "im.chat.access_event_v1"},
            "event": {}
        });
        assert!(decode_json(other_event, &inst).unwrap().is_none());
    }

    #[test]
    fn empty_payload_is_none_but_bad_schema_is_error() {
        let inst = installation("ou_bot", None);
        let d = LarkJsonFrameDecoder::new();
        assert!(d.decode(b"", &inst).unwrap().is_none());

        let err = d.decode(b"not json", &inst).unwrap_err();
        assert!(err.to_string().contains("envelope"));

        let missing_event = serde_json::json!({
            "schema": "2.0",
            "header": {"event_id": "e", "event_type": "im.message.receive_v1"}
        });
        let err = decode_json(missing_event, &inst).unwrap_err();
        assert!(err.to_string().contains("empty event payload"));
    }

    #[test]
    fn legacy_event_callback_envelope_shape_is_accepted() {
        let inst = installation("ou_bot", None);
        let legacy = serde_json::json!({
            "type": "event_callback",
            "header": {"event_id": "e", "event_type": "im.message.receive_v1"},
            "event": {
                "message": {"message_id": "om", "chat_id": "oc", "chat_type": "p2p",
                             "message_type": "text", "content": "{\"text\":\"x\"}"}
            }
        });
        assert!(decode_json(legacy, &inst).unwrap().is_some());
    }

    #[test]
    fn resolve_mentions_longest_key_wins() {
        let mentions = vec![
            LarkMention {
                key: "@_user_1".into(),
                name: "One".into(),
                ..Default::default()
            },
            LarkMention {
                key: "@_user_10".into(),
                name: "Ten".into(),
                ..Default::default()
            },
        ];
        assert_eq!(
            resolve_mentions("a @_user_10 b @_user_1 c", &mentions, "", ""),
            "a @Ten b @One c"
        );
    }

    #[test]
    fn resolve_mentions_strips_bot_with_whitespace_fidelity() {
        let bot = vec![LarkMention {
            key: "@_user_1".into(),
            id: LarkMentionId {
                open_id: "ou_bot".into(),
                ..Default::default()
            },
            name: "Patchbay".into(),
        }];
        // Trailing space eaten after the placeholder.
        assert_eq!(resolve_mentions("@_user_1 hi", &bot, "ou_bot", ""), "hi");
        // No trailing space → backtrack the emitted leading space.
        assert_eq!(resolve_mentions(" @_user_1", &bot, "ou_bot", ""), "");
        // Tab after the placeholder is preserved verbatim.
        assert_eq!(resolve_mentions("@_user_1\thi", &bot, "ou_bot", ""), "\thi");
    }

    #[test]
    fn resolve_mentions_unknown_name_keeps_placeholder() {
        let ms = vec![LarkMention {
            key: "@_user_2".into(),
            ..Default::default()
        }];
        assert_eq!(
            resolve_mentions("hey @_user_2!", &ms, "", ""),
            "hey @_user_2!"
        );
    }

    #[test]
    fn resolve_mentions_noop_on_empty_inputs() {
        assert_eq!(resolve_mentions("", &[], "", ""), "");
        assert_eq!(resolve_mentions("plain", &[], "", ""), "plain");
    }

    #[test]
    fn contains_mention_match_order() {
        let ms = vec![LarkMention {
            key: "@_u".into(),
            id: LarkMentionId {
                open_id: "ou_x".into(),
                union_id: "un_x".into(),
                user_id: String::new(),
            },
            name: String::new(),
        }];
        // Union id known: exclusive match.
        assert!(contains_mention(&ms, "other", "un_x"));
        assert!(!contains_mention(&ms, "ou_x", "un_other"));
        // Union id unknown: open_id fallback.
        assert!(contains_mention(&ms, "ou_x", ""));
        // Both blank: false.
        assert!(!contains_mention(&ms, "", ""));
        assert!(!contains_mention(&[], "ou_x", ""));
    }

    #[test]
    fn normalize_chat_type_maps_known_values() {
        assert_eq!(normalize_chat_type("p2p"), chat_type_p2p());
        assert_eq!(normalize_chat_type("GROUP"), chat_type_group());
        assert_eq!(
            normalize_chat_type("weird"),
            patchbay_channel::message::ChatType("weird".into())
        );
    }

    #[test]
    fn extract_text_body_shared_with_flattener() {
        assert_eq!(extract_text_body(r#"{"text":"t"}"#), "t");
    }
}
