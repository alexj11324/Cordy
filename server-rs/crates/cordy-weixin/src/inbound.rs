use serde::{Deserialize, Serialize};

use cordy_channel::{ChatType, InboundMessage, MsgType, Source};
use cordy_channel_engine::parse_fresh_session_command;

use crate::api::WeixinMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeixinRawEvent {
    pub bot_id: String,
    pub event_type: String,
    pub context_token: String,
}

pub fn inbound_from_message(message: &WeixinMessage, bot_id: &str) -> Option<InboundMessage> {
    if message.message_type != 1
        || message.from_user_id.is_empty()
        || message.from_user_id == bot_id
        || message.context_token.is_empty()
    {
        return None;
    }
    let text = message
        .item_list
        .iter()
        .filter(|item| item.r#type == 1)
        .filter_map(|item| item.text_item.as_ref())
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    let command_text = text.clone();
    let (text, force_fresh) = match parse_fresh_session_command(&text) {
        Some(body) => (body, true),
        None => (text, false),
    };
    // Tencent's reference channel currently declares direct chats only.
    // Do not imply group support from an observed group_id until the protocol
    // contract and addressing semantics are documented and tested.
    if !message.group_id.is_empty() {
        return None;
    }
    let chat_type = ChatType::p2p();
    let chat_id = message.from_user_id.clone();
    let message_id = if message.message_id != 0 {
        message.message_id.to_string()
    } else if !message.client_id.is_empty() {
        message.client_id.clone()
    } else {
        format!("{}:{}", message.seq, message.from_user_id)
    };
    Some(InboundMessage {
        event_id: message.seq.to_string(),
        message_id,
        source: Source {
            channel_type: cordy_channel::Type(crate::TYPE_WEIXIN.to_string()),
            chat_id,
            chat_type: chat_type.clone(),
            sender_id: message.from_user_id.clone(),
            sender_stable_id: message.from_user_id.clone(),
            thread_id: String::new(),
        },
        r#type: MsgType::text(),
        text,
        command_text,
        addressed_to_bot: chat_type == ChatType::p2p(),
        force_fresh,
        raw: serde_json::to_value(WeixinRawEvent {
            bot_id: bot_id.to_string(),
            event_type: "message".to_string(),
            context_token: message.context_token.clone(),
        })
        .unwrap_or_default(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{MessageItem, TextItem};

    #[test]
    fn normalizes_direct_text_and_keeps_context_token_private_to_raw() {
        let message = WeixinMessage {
            seq: 7,
            message_id: 9,
            from_user_id: "u@im.wechat".into(),
            message_type: 1,
            context_token: "ctx".into(),
            item_list: vec![MessageItem {
                r#type: 1,
                text_item: Some(TextItem {
                    text: " hello ".into(),
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = inbound_from_message(&message, "bot@im.bot").unwrap();
        assert_eq!(result.text, "hello");
        assert_eq!(result.source.chat_type, ChatType::p2p());
        assert_eq!(result.raw["context_token"], "ctx");
    }
}
