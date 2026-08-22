//! Port of `ws_frame.go`: DingTalk Stream frames are JSON envelopes over the
//! WebSocket. This module models the inbound DataFrame and the
//! DataFrameResponse we write back, plus the small set of type/topic
//! discriminators the chatbot connection uses. It replaces the former
//! dependency on the vendor stream SDK's payload package.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const FRAME_TYPE_SYSTEM: &str = "SYSTEM";
pub const FRAME_TYPE_CALLBACK: &str = "CALLBACK";

pub const SYSTEM_TOPIC_PING: &str = "ping";
pub const SYSTEM_TOPIC_DISCONNECT: &str = "disconnect";
pub const BOT_MESSAGE_TOPIC: &str = "/v1.0/im/bot/messages/get";

pub const FRAME_RESPONSE_CODE_OK: i32 = 200;

/// An inbound Stream frame. The gateway carries the routing key in
/// headers.topic and the correlation id in headers.messageId; data is the frame
/// body as a JSON string (a bot callback for the bot-message topic).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DataFrame {
    #[serde(rename = "type", default)]
    pub frame_type: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub data: String,
}

impl DataFrame {
    pub fn topic(&self) -> &str {
        self.headers.get("topic").map(String::as_str).unwrap_or("")
    }

    pub fn message_id(&self) -> &str {
        self.headers
            .get("messageId")
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// The ack we write back for every frame. The gateway correlates it to the
/// delivered frame by the echoed messageId header, so that header is
/// load-bearing; data stays empty for a plain callback ack (the actual reply is
/// delivered out-of-band over the Open API).
#[derive(Debug, Clone, Serialize)]
pub struct DataFrameResponse {
    pub code: i32,
    pub headers: HashMap<String, String>,
    pub message: String,
    pub data: String,
}

/// Builds a 200 ack echoing the frame's messageId.
pub fn new_ack_response(message_id: &str) -> DataFrameResponse {
    let mut headers = HashMap::new();
    headers.insert("messageId".to_string(), message_id.to_string());
    headers.insert("contentType".to_string(), "application/json".to_string());
    DataFrameResponse {
        code: FRAME_RESPONSE_CODE_OK,
        message: String::new(),
        data: String::new(),
        headers,
    }
}

/// Answers a SYSTEM ping. It echoes the ping's messageId and its data body,
/// mirroring the gateway's expected pong shape.
pub fn new_pong_response(message_id: &str, data: &str) -> DataFrameResponse {
    let mut resp = new_ack_response(message_id);
    resp.message = "ok".to_string();
    resp.data = data.to_string();
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_frame_with_headers() {
        let raw = json!({
            "type": "CALLBACK",
            "headers": {"topic": "/v1.0/im/bot/messages/get", "messageId": "m-1"},
            "data": "{\"msgtype\":\"text\"}"
        });
        let f: DataFrame = serde_json::from_value(raw).unwrap();
        assert_eq!(f.frame_type, "CALLBACK");
        assert_eq!(f.topic(), "/v1.0/im/bot/messages/get");
        assert_eq!(f.message_id(), "m-1");
        assert_eq!(f.data, "{\"msgtype\":\"text\"}");
    }

    #[test]
    fn missing_headers_default_empty() {
        let f: DataFrame = serde_json::from_value(json!({"type": "SYSTEM"})).unwrap();
        assert_eq!(f.topic(), "");
        assert_eq!(f.message_id(), "");
    }

    #[test]
    fn ack_echoes_message_id() {
        let r = new_ack_response("abc");
        assert_eq!(r.code, 200);
        assert_eq!(r.headers.get("messageId").map(String::as_str), Some("abc"));
        assert_eq!(
            r.headers.get("contentType").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(r.data, "");
    }

    #[test]
    fn pong_echoes_data_and_ok_message() {
        let r = new_pong_response("p1", "{\"ping\":1}");
        assert_eq!(r.code, 200);
        assert_eq!(r.message, "ok");
        assert_eq!(r.data, "{\"ping\":1}");
        assert_eq!(r.headers.get("messageId").map(String::as_str), Some("p1"));
    }
}
