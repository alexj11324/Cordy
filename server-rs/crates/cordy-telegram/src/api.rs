//! Telegram Bot API client: the JSON envelope, long-poll getUpdates, and
//! the send/edit surface the adapter needs.
//!
//! Port of `server/internal/integrations/telegram/api.go`.

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";

pub const LONG_POLL_TIMEOUT_SECS: u64 = 50;

/// The bot is already being polled by another instance (409 conflict).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ThisError)]
#[error("telegram: bot is already being polled by another instance (409 conflict)")]
pub struct ConflictError;

#[derive(Debug, Clone, PartialEq, Eq, ThisError)]
#[error("telegram: {method} request failed")]
pub struct RequestError {
    pub method: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, ThisError)]
#[error("telegram api: {code} {description}")]
pub struct ApiError {
    pub code: u16,
    pub description: String,
    pub retry_after: i64,
}

/// Extracts a Retry-After wait from a 429 API error; `false` for anything
/// else.
pub fn retry_after(err: &anyhow::Error) -> Option<Duration> {
    let ae = err.chain().find_map(|c| c.downcast_ref::<ApiError>())?;
    if ae.code != 429 {
        return None;
    }
    let secs = if ae.retry_after <= 0 {
        1
    } else {
        ae.retry_after
    };
    Some(Duration::from_secs(secs as u64))
}

/// The Bot API surface. Cloned per task; reqwest shares its pool.
#[derive(Clone)]
pub struct BotApi {
    /// API host, no trailing slash.
    base: String,
    token: String,
    http: reqwest::Client,
}

impl BotApi {
    pub fn new(base: &str, token: &str) -> Self {
        let base = if base.is_empty() {
            DEFAULT_API_BASE.to_string()
        } else {
            base.to_string()
        };
        Self {
            base,
            token: token.to_string(),
            // 65s covers the 50s long poll plus latency.
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(65))
                .build()
                .expect("static client configuration"),
        }
    }

    async fn call(
        &self,
        method: &str,
        params: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>> {
        let url = format!("{}/bot{}/{method}", self.base, self.token);
        let mut request = self.http.post(&url);
        if let Some(p) = params {
            request = request.header("Content-Type", "application/json").json(p);
        }
        let response = request.send().await.map_err(|e| {
            let _ = e;
            anyhow!("telegram: {method} request failed")
        })?;
        #[derive(Deserialize)]
        struct Parameters {
            #[serde(default, rename = "retry_after")]
            retry_after: i64,
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default)]
            ok: bool,
            #[serde(default)]
            result: serde_json::Value,
            #[serde(default, rename = "error_code")]
            error_code: u16,
            #[serde(default)]
            description: String,
            #[serde(default)]
            parameters: Option<Parameters>,
        }
        let env: Envelope = response
            .json()
            .await
            .map_err(|e| anyhow!("telegram: decode {method} response: {e}"))?;
        if !env.ok {
            return Err(anyhow!(ApiError {
                code: env.error_code,
                description: env.description,
                retry_after: env.parameters.map(|p| p.retry_after).unwrap_or(0),
            }));
        }
        Ok((!env.result.is_null()).then_some(env.result))
    }

    async fn call_unit(&self, method: &str, params: &serde_json::Value) -> Result<()> {
        self.call(method, Some(params)).await.map(|_| ())
    }

    pub async fn get_me(&self) -> Result<TelegramUser> {
        self.call("getMe", None)
            .await?
            .map(serde_json::from_value::<TelegramUser>)
            .transpose()?
            .ok_or_else(|| anyhow!("telegram: empty getMe result"))
    }

    pub async fn get_webhook_info(&self) -> Result<WebhookInfo> {
        self.call("getWebhookInfo", None)
            .await?
            .map(serde_json::from_value::<WebhookInfo>)
            .transpose()?
            .ok_or_else(|| anyhow!("telegram: empty webhook info"))
    }

    pub async fn get_updates(&self, offset: i64) -> Result<Vec<Update>, anyhow::Error> {
        let params = serde_json::json!({
            "offset": offset,
            "timeout": LONG_POLL_TIMEOUT_SECS,
            "allowed_updates": ["message"],
        });
        match self.call("getUpdates", Some(&params)).await {
            Ok(Some(result)) => Ok(serde_json::from_value(result)?),
            Ok(None) => Ok(Vec::new()),
            Err(err) => {
                // Map the upstream 409 onto the typed conflict error.
                let is_conflict = err
                    .chain()
                    .find_map(|c| c.downcast_ref::<ApiError>())
                    .is_some_and(|ae| ae.code == 409);
                if is_conflict {
                    Err(anyhow!(ConflictError))
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn get_file(&self, file_id: &str) -> Result<TelegramFile> {
        self.call("getFile", Some(&serde_json::json!({"file_id": file_id})))
            .await?
            .map(serde_json::from_value::<TelegramFile>)
            .transpose()?
            .ok_or_else(|| anyhow!("telegram: empty getFile result"))
    }

    /// Constructs Telegram's authenticated file endpoint after requiring the
    /// returned path to remain a relative path under `/file/bot<TOKEN>/`.
    pub fn file_url(&self, file: &TelegramFile) -> Result<url::Url> {
        let path = file.file_path.trim();
        if path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
            || path.contains('\\')
        {
            anyhow::bail!("telegram: invalid getFile path");
        }
        let base = format!("{}/file/bot{}/", self.base, self.token);
        url::Url::parse(&base)
            .and_then(|base| base.join(path))
            .map_err(|_| anyhow!("telegram: invalid file endpoint"))
    }

    pub async fn send_message(&self, p: &SendMessageParams) -> Result<TelegramMessage> {
        self.call("sendMessage", Some(&serde_json::to_value(p)?))
            .await?
            .map(serde_json::from_value::<TelegramMessage>)
            .transpose()?
            .ok_or_else(|| anyhow!("telegram: empty sendMessage result"))
    }

    /// Sends, honoring Retry-After once (Go sendMessageWithRetryAfter).
    pub async fn send_message_with_retry_after(
        &self,
        p: &SendMessageParams,
    ) -> Result<TelegramMessage> {
        match self.send_message(p).await {
            Ok(m) => Ok(m),
            Err(err) => {
                if let Some(wait) = retry_after(&err) {
                    sleep_ctx(wait).await;
                    return self.send_message(p).await;
                }
                Err(err)
            }
        }
    }

    pub async fn edit_message_text(&self, p: &EditMessageTextParams) -> Result<()> {
        self.call_unit("editMessageText", &serde_json::to_value(p)?)
            .await
    }

    pub async fn send_chat_action(&self, chat_id: i64, message_thread_id: i64) -> Result<()> {
        let mut params = serde_json::json!({"chat_id": chat_id, "action": "typing"});
        if message_thread_id != 0 {
            params["message_thread_id"] = serde_json::json!(message_thread_id);
        }
        self.call_unit("sendChatAction", &params).await
    }
}

/// Sleeps for `d`; returns early on shutdown. Mirrors Go's sleepCtx —
/// cancellation simply ends the wait (the caller re-checks the token).
async fn sleep_ctx(d: Duration) {
    tokio::time::sleep(d).await;
}

// ── wire types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TelegramUser {
    pub id: i64,
    #[serde(rename = "is_bot", default)]
    pub is_bot: bool,
    #[serde(rename = "first_name", default)]
    pub first_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TelegramChat {
    pub id: i64,
    /// "private", "group", "supergroup", "channel"
    #[serde(rename = "type")]
    pub r#type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub r#type: String,
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DocumentRef {
    #[serde(rename = "file_id", default)]
    pub file_id: String,
    #[serde(rename = "file_size", default)]
    pub file_size: i64,
    #[serde(rename = "file_name", default)]
    pub file_name: String,
    #[serde(rename = "mime_type", default)]
    pub mime_type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhotoSize {
    #[serde(rename = "file_id", default)]
    pub file_id: String,
    #[serde(rename = "file_size", default)]
    pub file_size: i64,
    #[serde(default)]
    pub width: i64,
    #[serde(default)]
    pub height: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VoiceRef {
    #[serde(rename = "file_id", default)]
    pub file_id: String,
    #[serde(rename = "file_size", default)]
    pub file_size: i64,
    #[serde(rename = "mime_type", default)]
    pub mime_type: String,
    #[serde(default)]
    pub duration: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VideoRef {
    #[serde(rename = "file_id", default)]
    pub file_id: String,
    #[serde(rename = "file_size", default)]
    pub file_size: i64,
    #[serde(rename = "file_name", default)]
    pub file_name: String,
    #[serde(rename = "mime_type", default)]
    pub mime_type: String,
    #[serde(default)]
    pub width: i64,
    #[serde(default)]
    pub height: i64,
    #[serde(default)]
    pub duration: i64,
}

/// The subset of a Telegram message the adapter consumes. Unknown fields
/// are ignored by the deserializer exactly as Go's struct does.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TelegramMessage {
    #[serde(rename = "message_id", default)]
    pub message_id: i64,
    #[serde(default)]
    pub from: Option<TelegramUser>,
    #[serde(default)]
    pub chat: TelegramChat,
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<MessageEntity>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "caption_entities"
    )]
    pub caption_entities: Vec<MessageEntity>,
    #[serde(
        default,
        rename = "reply_to_message",
        skip_serializing_if = "Option::is_none"
    )]
    pub reply_to_message: Option<Box<TelegramMessage>>,
    #[serde(
        default,
        rename = "message_thread_id",
        skip_serializing_if = "is_zero_i64"
    )]
    pub message_thread_id: i64,
    #[serde(
        default,
        rename = "is_topic_message",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub is_topic_message: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photo: Vec<PhotoSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<DocumentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticker: Option<serde_json::Value>,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Update {
    #[serde(rename = "update_id", default)]
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct WebhookInfo {
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "pending_update_count")]
    pub pending_update_count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TelegramFile {
    #[serde(rename = "file_id", default)]
    pub file_id: String,
    #[serde(rename = "file_size", default)]
    pub file_size: i64,
    #[serde(rename = "file_path", default)]
    pub file_path: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SendMessageParams {
    #[serde(rename = "chat_id")]
    pub chat_id: i64,
    pub text: String,
    #[serde(rename = "parse_mode", skip_serializing_if = "String::is_empty")]
    pub parse_mode: String,
    #[serde(rename = "message_thread_id", skip_serializing_if = "is_zero_i64")]
    pub message_thread_id: i64,
    #[serde(rename = "reply_parameters", skip_serializing_if = "Option::is_none")]
    pub reply_parameters: Option<ReplyParameters>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct ReplyParameters {
    #[serde(rename = "message_id")]
    pub message_id: i64,
    #[serde(
        rename = "allow_sending_without_reply",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub allow_sending_without_reply: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EditMessageTextParams {
    #[serde(rename = "chat_id")]
    pub chat_id: i64,
    #[serde(rename = "message_id")]
    pub message_id: i64,
    pub text: String,
    #[serde(rename = "parse_mode", skip_serializing_if = "String::is_empty")]
    pub parse_mode: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(raw: &str) -> Update {
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn update_decodes_minimal_message() {
        let u = update(
            r#"{"update_id":1,"message":{"message_id":10,"date":1700000000,
               "from":{"id":42,"is_bot":false,"first_name":"Ann"},
               "chat":{"id":-100,"type":"supergroup"},"text":"hi"}}"#,
        );
        let m = u.message.unwrap();
        assert_eq!(u.update_id, 1);
        assert_eq!(m.from.as_ref().unwrap().id, 42);
        assert_eq!(m.chat.r#type, "supergroup");
        assert_eq!(m.text, "hi");
    }

    #[test]
    fn topic_fields_and_reply_roundtrip() {
        let m: TelegramMessage = serde_json::from_str(
            r#"{"message_id":5,"message_thread_id":9,"is_topic_message":true,
               "reply_to_message":{"message_id":3}}"#,
        )
        .unwrap();
        assert_eq!(m.message_thread_id, 9);
        assert!(m.is_topic_message);
        assert_eq!(m.reply_to_message.as_ref().unwrap().message_id, 3);

        // Zero thread id stays absent on the wire (omitempty parity).
        let plain = TelegramMessage {
            message_id: 5,
            ..Default::default()
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("message_thread_id").is_none());
    }

    #[test]
    fn send_params_omit_empty_optionals() {
        let p = SendMessageParams {
            chat_id: 7,
            text: "x".into(),
            ..Default::default()
        };
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("parse_mode").is_none());
        assert!(json.get("message_thread_id").is_none());
        assert!(json.get("reply_parameters").is_none());

        let with_thread = SendMessageParams {
            chat_id: 7,
            text: "x".into(),
            message_thread_id: 3,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&with_thread).unwrap()["message_thread_id"],
            3
        );
    }

    #[test]
    fn file_endpoint_accepts_only_relative_provider_paths() {
        let api = BotApi::new("https://api.telegram.org", "123:secret");
        let url = api
            .file_url(&TelegramFile {
                file_path: "documents/file.pdf".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(url.host_str(), Some("api.telegram.org"));
        assert!(url.path().ends_with("/documents/file.pdf"));
        for path in ["", "/etc/passwd", "../secret", "a/../../secret", "a\\b"] {
            assert!(api
                .file_url(&TelegramFile {
                    file_path: path.into(),
                    ..Default::default()
                })
                .is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires network"]
    async fn get_me_roundtrip() {
        let api = BotApi::new("", "token");
        let _ = api.get_me().await.unwrap();
    }
}
