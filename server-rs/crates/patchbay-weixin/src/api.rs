use std::time::Duration;

use base64::Engine as _;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BOT_AGENT: &str = "Patchbay/0.1.0";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct QrCodeResponse {
    pub qrcode: String,
    #[serde(default)]
    pub qrcode_img_content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct QrStatusResponse {
    pub status: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub ilink_bot_id: String,
    #[serde(default)]
    pub ilink_user_id: String,
    #[serde(default)]
    pub baseurl: String,
    #[serde(default)]
    pub redirect_host: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BaseInfo {
    pub channel_version: String,
    pub bot_agent: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TextItem {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MessageItem {
    #[serde(default)]
    pub r#type: i32,
    #[serde(default)]
    pub msg_id: String,
    #[serde(default)]
    pub text_item: Option<TextItem>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WeixinMessage {
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub message_id: i64,
    #[serde(default)]
    pub from_user_id: String,
    #[serde(default)]
    pub to_user_id: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub group_id: String,
    #[serde(default)]
    pub message_type: i32,
    #[serde(default)]
    pub message_state: i32,
    #[serde(default)]
    pub item_list: Vec<MessageItem>,
    #[serde(default)]
    pub context_token: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GetUpdatesResponse {
    #[serde(default)]
    pub ret: i32,
    #[serde(default)]
    pub errcode: i32,
    #[serde(default)]
    pub errmsg: String,
    #[serde(default)]
    pub msgs: Vec<WeixinMessage>,
    #[serde(default)]
    pub get_updates_buf: String,
    #[serde(default)]
    pub longpolling_timeout_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    ret: i32,
    #[serde(default)]
    errmsg: String,
}

#[derive(Debug, thiserror::Error)]
#[error("weixin API {operation} failed: ret={ret} {message}")]
pub struct ApiError {
    pub operation: &'static str,
    pub ret: i32,
    pub message: String,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl Client {
    pub fn new(base_url: &str, token: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(40))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            base_url: normalized_base_url(base_url),
            token: token.trim().to_string(),
        })
    }

    pub async fn request_qr_code(local_tokens: &[String]) -> anyhow::Result<QrCodeResponse> {
        let client = Self::new(DEFAULT_BASE_URL, "")?;
        let response = client
            .post("ilink/bot/get_bot_qrcode?bot_type=3")
            .json(&serde_json::json!({"local_token_list": local_tokens}))
            .send()
            .await?
            .error_for_status()?;
        let qr = response.json::<QrCodeResponse>().await?;
        if qr.qrcode.trim().is_empty() {
            anyhow::bail!("weixin: QR response was incomplete");
        }
        Ok(qr)
    }

    pub async fn qr_status(
        base_url: &str,
        qrcode: &str,
        verify_code: Option<&str>,
    ) -> anyhow::Result<QrStatusResponse> {
        let client = Self::new(base_url, "")?;
        let mut request = client
            .get("ilink/bot/get_qrcode_status")
            .query(&[("qrcode", qrcode)]);
        if let Some(code) = verify_code.filter(|value| !value.trim().is_empty()) {
            request = request.query(&[("verify_code", code)]);
        }
        Ok(request
            .send()
            .await?
            .error_for_status()?
            .json::<QrStatusResponse>()
            .await?)
    }

    pub async fn get_updates(&self, cursor: &str) -> anyhow::Result<GetUpdatesResponse> {
        let response = self
            .post("ilink/bot/getupdates")
            .json(&serde_json::json!({
                "get_updates_buf": cursor,
                "base_info": base_info(),
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<GetUpdatesResponse>()
            .await?;
        if response.ret != 0 || response.errcode != 0 {
            return Err(ApiError {
                operation: "getupdates",
                ret: if response.errcode != 0 {
                    response.errcode
                } else {
                    response.ret
                },
                message: response.errmsg.clone(),
            }
            .into());
        }
        Ok(response)
    }

    pub async fn send_text(
        &self,
        to_user_id: &str,
        context_token: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        for chunk in chunk_text(text, 2_000) {
            let response = self
                .post("ilink/bot/sendmessage")
                .json(&serde_json::json!({
                    "msg": {
                        "client_id": uuid::Uuid::now_v7().to_string(),
                        "from_user_id": "",
                        "to_user_id": to_user_id,
                        "message_type": 2,
                        "message_state": 2,
                        "context_token": context_token,
                        "item_list": [{"type": 1, "text_item": {"text": chunk}}],
                    },
                    "base_info": base_info(),
                }))
                .send()
                .await?
                .error_for_status()?
                .json::<ApiResponse>()
                .await?;
            if response.ret != 0 {
                return Err(ApiError {
                    operation: "sendmessage",
                    ret: response.ret,
                    message: response.errmsg,
                }
                .into());
            }
        }
        Ok(())
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.headers(self.http.post(format!("{}/{}", self.base_url, path)))
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.common_headers(self.http.get(format!("{}/{}", self.base_url, path)))
    }

    fn common_headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", encoded_client_version())
    }

    fn headers(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut request = self
            .common_headers(request)
            .header("AuthorizationType", "ilink_bot_token")
            .header("X-WECHAT-UIN", random_wechat_uin());
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        request
    }
}

fn normalized_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        value.to_string()
    }
}

fn random_wechat_uin() -> String {
    let value = rand::thread_rng().next_u32().to_string();
    base64::engine::general_purpose::STANDARD.encode(value)
}

fn encoded_client_version() -> u32 {
    let mut parts = CLIENT_VERSION
        .split('.')
        .map(|value| value.parse::<u32>().unwrap_or(0));
    ((parts.next().unwrap_or(0) & 0xff) << 16)
        | ((parts.next().unwrap_or(0) & 0xff) << 8)
        | (parts.next().unwrap_or(0) & 0xff)
}

fn base_info() -> BaseInfo {
    BaseInfo {
        channel_version: CLIENT_VERSION.to_string(),
        bot_agent: BOT_AGENT.to_string(),
    }
}

fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for ch in text.chars() {
        if count == limit {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_client_version_are_stable() {
        assert_eq!(normalized_base_url(""), DEFAULT_BASE_URL);
        assert_eq!(
            normalized_base_url("https://example.test/"),
            "https://example.test"
        );
        assert!(encoded_client_version() > 0);
        assert!(!random_wechat_uin().is_empty());
        assert_eq!(chunk_text(&"你".repeat(2_001), 2_000).len(), 2);
    }

    #[test]
    fn protocol_payload_decodes_unknown_fields_forward_compatibly() {
        let response: GetUpdatesResponse = serde_json::from_value(serde_json::json!({
            "ret": 0,
            "future": true,
            "msgs": [{
                "message_id": 42,
                "message_type": 1,
                "from_user_id": "u@im.wechat",
                "item_list": [{"type": 1, "text_item": {"text": "hello"}, "new": 1}]
            }]
        }))
        .unwrap();
        assert_eq!(
            response.msgs[0].item_list[0]
                .text_item
                .as_ref()
                .unwrap()
                .text,
            "hello"
        );
    }
}
