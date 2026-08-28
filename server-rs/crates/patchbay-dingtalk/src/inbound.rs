//! Port of `inbound.go`: translation from a DingTalk Stream callback
//! ([`BotCallbackData`]) to the engine's normalized
//! [`patchbay_channel::InboundMessage`]. The per-installation connection
//! ([`crate::dingtalk_channel`]) threads in its OWN installation's AppKey so
//! the resolver can route the event back to its installation — DingTalk's
//! callback payload does not carry the robot code itself.

use serde::{Deserialize, Serialize};

use patchbay_channel::{ChatType, InboundMessage, MsgType, Source};
use patchbay_channel_engine::parse_fresh_session_command;

use crate::channel_type;

/// The DingTalk bot-message callback payload — the JSON carried in a CALLBACK
/// frame's data field. It holds only the fields the translation reads; DingTalk
/// sends more, which we ignore. Replaces the vendor SDK's
/// chatbot.BotCallbackDataModel.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BotCallbackData {
    #[serde(rename = "conversationId", default)]
    pub conversation_id: String,
    #[serde(rename = "conversationTitle", default)]
    pub conversation_title: String,
    #[serde(rename = "conversationType", default)]
    pub conversation_type: String,
    #[serde(rename = "senderStaffId", default)]
    pub sender_staff_id: String,
    #[serde(rename = "msgId", default)]
    pub msg_id: String,
    #[serde(rename = "msgtype", default)]
    pub msgtype: String,
    #[serde(rename = "isInAtList", default)]
    pub is_in_at_list: bool,
    #[serde(default)]
    pub text: BotCallbackText,
    /// The msgtype-discriminated payload of non-text messages (picture /
    /// richText). Decoded lazily per msgtype; absent on over-quota callbacks
    /// (errorCode 20001 strips text/content entirely).
    #[serde(default)]
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BotCallbackText {
    #[serde(default)]
    pub content: String,
}

/// The content shape of msgtype=picture. Real payloads carry both codes
/// (developerpedia); either resolves through messageFiles/download.
#[derive(Debug, Clone, Default, Deserialize)]
struct PictureContent {
    #[serde(default, rename = "downloadCode")]
    download_code: String,
    #[serde(default, rename = "pictureDownloadCode")]
    picture_download_code: String,
}

/// The content shape of msgtype=richText: an ORDERED array of heterogeneous
/// items — text runs {"text":…} interleaved with picture items
/// {"type":"picture","downloadCode":…} in send order. Item kinds beyond
/// text/picture are undocumented today and skipped.
#[derive(Debug, Clone, Default, Deserialize)]
struct RichTextContent {
    #[serde(default, rename = "richText")]
    rich_text: Vec<RichTextItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RichTextItem {
    #[serde(default)]
    text: String,
    #[serde(rename = "type", default)]
    item_type: String,
    #[serde(default, rename = "downloadCode")]
    download_code: String,
    #[serde(default, rename = "pictureDownloadCode")]
    picture_download_code: String,
}

impl RichTextItem {
    fn is_picture(&self) -> bool {
        self.item_type == "picture"
            || !self.download_code.is_empty()
            || !self.picture_download_code.is_empty()
    }
}

/// Orders a picture item's two download codes into (primary, fallback),
/// promoting the secondary code when the primary is missing.
fn ref_alt<'a>(download_code: &'a str, picture_download_code: &'a str) -> (&'a str, &'a str) {
    if !download_code.is_empty() {
        return (download_code, picture_download_code);
    }
    (picture_download_code, "")
}

/// Carries the DingTalk-specific fields the cross-platform envelope does not.
/// `app_id` is stamped by the receiving connection (it is the installation's
/// routing key) and read back only inside the resolvers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DingtalkRawEvent {
    #[serde(rename = "app_id")]
    pub app_id: String,
    #[serde(
        rename = "conversation_title",
        skip_serializing_if = "String::is_empty"
    )]
    pub conversation_title: String,
    #[serde(rename = "media", skip_serializing_if = "Vec::is_empty", default)]
    pub media: Vec<DingtalkMediaResource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DingtalkMediaResource {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(rename = "alt", skip_serializing_if = "String::is_empty", default)]
    pub alt: String,
    /// The occurrence of the adapter-generated marker in the visible body,
    /// including identical user-authored text.
    #[serde(rename = "inline_index", skip_serializing_if = "is_zero", default)]
    pub inline_index: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

pub(crate) fn dingtalk_media_resource_at(
    reference: impl Into<String>,
    alt: impl Into<String>,
    inline_index: usize,
) -> DingtalkMediaResource {
    DingtalkMediaResource {
        reference: reference.into(),
        alt: alt.into(),
        inline_index,
    }
}

/// Conversation type discriminators DingTalk sends in conversationType.
pub const CONV_TYPE_P2P: &str = "1";
pub const CONV_TYPE_GROUP: &str = "2";
pub const DINGTALK_IMAGE_PLACEHOLDER: &str = "[Image]";

/// Normalizes a DingTalk bot callback. It returns None only for events that
/// must not reach the core at all: messages with no sender staff id (system /
/// bot-authored). Text, picture and richText become ingestable messages; a
/// malformed/over-quota media payload (the 20001 shape strips content) still
/// reaches the core as an explicit unavailable-image placeholder rather than
/// the adapter dropping it silently; audio/video/file/unknown kinds likewise
/// pass through as text placeholders. A direct (1:1) message is always
/// addressed to the bot; a group message reaches the bot only when it carries
/// an @-mention of it, which DingTalk reports via isInAtList.
pub fn inbound_from_callback(data: &BotCallbackData, app_id: &str) -> Option<InboundMessage> {
    if data.sender_staff_id.is_empty() {
        return None;
    }

    let chat_type = dingtalk_chat_type(&data.conversation_type);
    let mut raw_event = DingtalkRawEvent {
        app_id: app_id.to_string(),
        conversation_title: data.conversation_title.trim().to_string(),
        media: Vec::new(),
    };
    let mut msg = InboundMessage {
        event_id: data.msg_id.clone(),
        message_id: data.msg_id.clone(),
        addressed_to_bot: chat_type == ChatType::p2p() || data.is_in_at_list,
        source: Source {
            channel_type: channel_type(),
            chat_id: data.conversation_id.clone(),
            chat_type,
            sender_id: data.sender_staff_id.clone(),
            ..Default::default()
        },
        ..Default::default()
    };

    match data.msgtype.as_str() {
        "text" => {
            msg.r#type = MsgType::text();
            msg.text = data.text.content.trim().to_string();
            msg.command_text.clone_from(&msg.text);
            Some(with_dingtalk_raw(msg, raw_event))
        }

        "picture" => {
            let pc: Option<PictureContent> = if data.content.is_null() {
                None
            } else {
                serde_json::from_value(data.content.clone()).ok()
            };
            let Some(pc) = pc else {
                // Over-quota (errorCode 20001 strips content) or malformed
                // payload: the sender is a real user who sent an image the bot
                // cannot read. Route it into the engine so it gets
                // identity-gated feedback.
                return Some(media_unreadable_msg(msg, raw_event));
            };
            let (reference, alt) = ref_alt(&pc.download_code, &pc.picture_download_code);
            if reference.is_empty() {
                return Some(media_unreadable_msg(msg, raw_event));
            }
            msg.r#type = MsgType::image();
            msg.text = DINGTALK_IMAGE_PLACEHOLDER.to_string();
            msg.command_text.clone_from(&msg.text);
            raw_event.media = vec![dingtalk_media_resource_at(reference, alt, 0)];
            Some(with_dingtalk_raw(msg, raw_event))
        }

        "richText" => {
            let rc: RichTextContent = if data.content.is_null() {
                // Over-quota / malformed richText: surface it to the engine for
                // identity-gated feedback rather than a silent adapter drop.
                return Some(media_unreadable_msg(msg, raw_event));
            } else {
                match serde_json::from_value(data.content.clone()) {
                    Ok(rc) => rc,
                    Err(_) => return Some(media_unreadable_msg(msg, raw_event)),
                }
            };
            let mut text = String::new();
            let mut command_text = String::new();
            let mut inline_placeholder_count: usize = 0;
            for item in &rc.rich_text {
                // A single item may in principle carry BOTH a text run and a
                // picture code; handle each independently (not a switch) so
                // neither is silently dropped. Text first, then image, matching
                // send order. Items with neither (undocumented kinds)
                // contribute nothing.
                if !item.text.is_empty() {
                    text.push_str(&item.text);
                    command_text.push_str(&item.text);
                    inline_placeholder_count +=
                        item.text.matches(DINGTALK_IMAGE_PLACEHOLDER).count();
                }
                if item.is_picture() {
                    let (reference, alt) =
                        ref_alt(&item.download_code, &item.picture_download_code);
                    if reference.is_empty() {
                        continue; // a picture item with no usable code
                    }
                    append_image_placeholder(&mut text);
                    raw_event.media.push(dingtalk_media_resource_at(
                        reference,
                        alt,
                        inline_placeholder_count,
                    ));
                    inline_placeholder_count += 1;
                }
            }
            msg.r#type = if raw_event.media.is_empty() {
                MsgType::text()
            } else {
                MsgType::image()
            };
            msg.text = text.trim().to_string();
            msg.command_text = command_text.trim().to_string();
            normalize_rich_text_fresh_layout(&mut msg, &rc.rich_text, !raw_event.media.is_empty());
            Some(with_dingtalk_raw(msg, raw_event))
        }

        "audio" => {
            msg.r#type = MsgType::audio();
            msg.text = "[Audio message]".to_string();
            msg.command_text.clone_from(&msg.text);
            Some(with_dingtalk_raw(msg, raw_event))
        }
        "video" => {
            msg.r#type = MsgType::video();
            msg.text = "[Video message]".to_string();
            msg.command_text.clone_from(&msg.text);
            Some(with_dingtalk_raw(msg, raw_event))
        }
        "file" => {
            msg.r#type = MsgType::file();
            msg.text = "[File]".to_string();
            msg.command_text.clone_from(&msg.text);
            Some(with_dingtalk_raw(msg, raw_event))
        }
        _ => {
            msg.r#type = MsgType::unknown();
            msg.text = "[Unsupported DingTalk message]".to_string();
            msg.command_text.clone_from(&msg.text);
            Some(with_dingtalk_raw(msg, raw_event))
        }
    }
}

/// Strips `/new` from the visible rich-text body before the shared Router
/// handles it, preserving interleaved image placeholders that Router cannot
/// reconstruct from CommandText. It deliberately keeps the original command
/// source, so `/new /issue ...` remains one /new command just like it does on
/// Lark and Slack; the adapter never reclassifies the remainder as a second
/// command.
fn normalize_rich_text_fresh_layout(
    msg: &mut InboundMessage,
    items: &[RichTextItem],
    has_media: bool,
) {
    let Some(body) = parse_fresh_session_command(&msg.command_text) else {
        return;
    };
    if body.is_empty() && !has_media {
        return;
    }

    let Some(first_text) = items.iter().position(|i| !i.text.trim().is_empty()) else {
        return;
    };
    let Some(first_body) = parse_fresh_session_command(&items[first_text].text) else {
        return;
    };

    msg.force_fresh = true;
    let mut visible = String::new();
    for (idx, item) in items.iter().enumerate() {
        // items[firstText].Text was replaced with the parsed /new body.
        if idx == first_text {
            visible.push_str(&first_body);
        } else {
            visible.push_str(&item.text);
        }
        if item.is_picture() {
            let (reference, _) = ref_alt(&item.download_code, &item.picture_download_code);
            if !reference.is_empty() {
                append_image_placeholder(&mut visible);
            }
        }
    }
    msg.text = visible.trim().to_string();
    if body.is_empty() {
        // A media-bearing `/new` is a real turn, not the shared bare-command
        // sentinel. ForceFresh carries the already-consumed directive.
        msg.command_text.clone_from(&msg.text);
    }
}

fn with_dingtalk_raw(mut msg: InboundMessage, raw_event: DingtalkRawEvent) -> InboundMessage {
    msg.raw = serde_json::to_value(raw_event).unwrap_or(serde_json::Value::Null);
    msg
}

/// Turns media the adapter cannot resolve into an explicit placeholder. With no
/// downloadable reference, the shared media resolver stays out of the path and
/// the normal channel turn carries the degradation signal.
fn media_unreadable_msg(mut msg: InboundMessage, raw_event: DingtalkRawEvent) -> InboundMessage {
    msg.r#type = MsgType::image();
    msg.text = "[Image unavailable]".to_string();
    msg.command_text.clone_from(&msg.text);
    with_dingtalk_raw(msg, raw_event)
}

fn append_image_placeholder(b: &mut String) {
    if !b.is_empty() {
        b.push('\n');
    }
    b.push_str(DINGTALK_IMAGE_PLACEHOLDER);
    b.push('\n');
}

/// Maps DingTalk's conversationType to the normalized ChatType. "1" is a 1:1
/// direct chat; everything else (group "2") is a group, which routes through
/// the engine's "must address the bot" filter.
pub fn dingtalk_chat_type(conversation_type: &str) -> ChatType {
    if conversation_type == CONV_TYPE_P2P {
        ChatType::p2p()
    } else {
        ChatType::group()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_data() -> BotCallbackData {
        BotCallbackData {
            conversation_id: "cid-1".into(),
            conversation_type: CONV_TYPE_P2P.into(),
            sender_staff_id: "staff-9".into(),
            msg_id: "m-1".into(),
            msgtype: "text".into(),
            ..Default::default()
        }
    }

    #[test]
    fn drops_messages_without_sender() {
        let mut d = base_data();
        d.sender_staff_id.clear();
        assert!(inbound_from_callback(&d, "app").is_none());
    }

    #[test]
    fn text_message_normalizes() {
        let mut d = base_data();
        d.text.content = "  hello world  ".into();
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.r#type, MsgType::text());
        assert_eq!(msg.text, "hello world");
        assert_eq!(msg.command_text, "hello world");
        assert!(msg.addressed_to_bot); // p2p always addressed
        assert_eq!(msg.source.chat_id, "cid-1");
        assert_eq!(msg.source.sender_id, "staff-9");
        assert_eq!(msg.raw["app_id"], json!("app"));
    }

    #[test]
    fn group_message_requires_at_mention() {
        let mut d = base_data();
        d.conversation_type = CONV_TYPE_GROUP.into();
        d.text.content = "hi".into();
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert!(!msg.addressed_to_bot);
        assert_eq!(msg.source.chat_type, ChatType::group());

        d.is_in_at_list = true;
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert!(msg.addressed_to_bot);
    }

    #[test]
    fn over_quota_picture_becomes_unavailable_placeholder() {
        let mut d = base_data();
        d.msgtype = "picture".into();
        d.content = serde_json::Value::Null; // errorCode 20001 shape
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.r#type, MsgType::image());
        assert_eq!(msg.text, "[Image unavailable]");
        assert!(msg.raw["media"].is_null()); // omitted when empty (Go omitempty)
    }

    #[test]
    fn picture_with_codes_carries_media_ref() {
        let mut d = base_data();
        d.msgtype = "picture".into();
        d.content = json!({"downloadCode": "dc1"});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.r#type, MsgType::image());
        assert_eq!(msg.text, "[Image]");
        assert_eq!(msg.raw["media"][0]["ref"], json!("dc1"));
        assert!(msg.raw["media"][0].get("inline_index").is_none()); // 0 omitted
    }

    #[test]
    fn picture_falls_back_to_secondary_code() {
        let mut d = base_data();
        d.msgtype = "picture".into();
        d.content = json!({"pictureDownloadCode": "pdc"});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.raw["media"][0]["ref"], json!("pdc"));
    }

    #[test]
    fn rich_text_interleaves_text_and_pictures_in_order() {
        let mut d = base_data();
        d.conversation_type = CONV_TYPE_GROUP.into();
        d.is_in_at_list = true;
        d.msgtype = "richText".into();
        d.content = json!({"richText": [
            {"text": "look at this"},
            {"type": "picture", "downloadCode": "dc-a"},
            {"text": "and this"},
            {"type": "picture", "pictureDownloadCode": "pdc-b"}
        ]});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.r#type, MsgType::image());
        assert_eq!(msg.text, "look at this\n[Image]\nand this\n[Image]");
        let media = msg.raw["media"].as_array().unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0]["ref"], json!("dc-a"));
        // inline_index 0 is omitted (Go omitempty); the first text run pushed
        // the counter to 0 for this picture.
        assert!(media[0].get("inline_index").is_none());
        assert_eq!(media[1]["ref"], json!("pdc-b"));
        assert_eq!(media[1]["inline_index"], json!(1));
    }

    #[test]
    fn rich_text_without_usable_pictures_is_plain_text() {
        let mut d = base_data();
        d.msgtype = "richText".into();
        d.content = json!({"richText": [{"text": "just words"}]});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.r#type, MsgType::text());
        assert_eq!(msg.text, "just words");
    }

    #[test]
    fn rich_text_new_with_media_strips_directive_keeps_placeholders() {
        let mut d = base_data();
        d.msgtype = "richText".into();
        d.content = json!({"richText": [
            {"text": "/new explain"},
            {"type": "picture", "downloadCode": "dc-x"}
        ]});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert!(msg.force_fresh);
        assert_eq!(msg.text, "explain\n[Image]");
        // body is non-empty ("explain"), so the original command source is
        // kept verbatim; ForceFresh carries the consumed directive.
        assert_eq!(msg.command_text, "/new explain");
    }

    #[test]
    fn rich_text_bare_new_with_media_mirrors_body_into_command_text() {
        let mut d = base_data();
        d.msgtype = "richText".into();
        d.content = json!({"richText": [
            {"text": "/new"},
            {"type": "picture", "downloadCode": "dc-x"}
        ]});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert!(msg.force_fresh);
        assert_eq!(msg.text, "[Image]");
        // A media-bearing bare /new is a real turn, not the shared
        // bare-command sentinel: command text mirrors the visible body.
        assert_eq!(msg.command_text, "[Image]");
    }

    #[test]
    fn rich_text_new_with_followup_issue_keeps_command_source() {
        let mut d = base_data();
        d.msgtype = "richText".into();
        d.content = json!({"richText": [
            {"text": "/new /issue Fix it"},
            {"text": "\nsteps below"}
        ]});
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert!(msg.force_fresh);
        assert_eq!(msg.text, "/issue Fix it\nsteps below");
        // Original command source preserved so /issue stays one command.
        assert_eq!(msg.command_text, "/new /issue Fix it\nsteps below");
    }

    #[test]
    fn unsupported_kinds_pass_through_as_placeholders() {
        for (msgtype, want) in [
            ("audio", ("[Audio message]", MsgType::audio())),
            ("video", ("[Video message]", MsgType::video())),
            ("file", ("[File]", MsgType::file())),
            (
                "hologram",
                ("[Unsupported DingTalk message]", MsgType::unknown()),
            ),
        ] {
            let mut d = base_data();
            d.msgtype = msgtype.to_string();
            let msg = inbound_from_callback(&d, "app").unwrap();
            assert_eq!(msg.text, want.0, "{msgtype}");
            assert_eq!(msg.r#type, want.1, "{msgtype}");
        }
    }

    #[test]
    fn conversation_title_is_trimmed_into_raw() {
        let mut d = base_data();
        d.conversation_title = "  My Group  ".into();
        let msg = inbound_from_callback(&d, "app").unwrap();
        assert_eq!(msg.raw["conversation_title"], json!("My Group"));
    }
}
