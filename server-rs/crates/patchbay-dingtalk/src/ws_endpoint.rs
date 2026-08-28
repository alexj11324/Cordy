//! Port of `ws_endpoint.go`: opens a DingTalk Stream connection — a POST to
//! the gateway that returns a single-use WebSocket endpoint + ticket. It
//! replaces the vendor SDK's client.Start handshake. The returned URL
//! (endpoint with the ticket appended) is what the connector dials.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const CONNECTIONS_OPEN_PATH: &str = "/v1.0/gateway/connections/open";
pub const STREAM_USER_AGENT: &str = "patchbay-dingtalk/1.0";
pub const OPEN_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// One {type, topic} entry in the open request. The chatbot connection
/// subscribes to the two SYSTEM control topics plus the bot-message callback
/// topic.
#[derive(Debug, Clone, Serialize)]
pub struct StreamSubscription {
    #[serde(rename = "type")]
    pub stream_type: String,
    pub topic: String,
}

/// The fixed subscription set for a bot-message stream.
pub fn chatbot_subscriptions() -> Vec<StreamSubscription> {
    use crate::ws_frame::*;
    vec![
        StreamSubscription {
            stream_type: FRAME_TYPE_SYSTEM.to_string(),
            topic: SYSTEM_TOPIC_PING.to_string(),
        },
        StreamSubscription {
            stream_type: FRAME_TYPE_SYSTEM.to_string(),
            topic: SYSTEM_TOPIC_DISCONNECT.to_string(),
        },
        StreamSubscription {
            stream_type: FRAME_TYPE_CALLBACK.to_string(),
            topic: BOT_MESSAGE_TOPIC.to_string(),
        },
    ]
}

#[derive(Debug, Serialize)]
struct OpenConnectionRequest<'a> {
    #[serde(rename = "clientId")]
    client_id: &'a str,
    #[serde(rename = "clientSecret")]
    client_secret: &'a str,
    subscriptions: Vec<StreamSubscription>,
    ua: &'a str,
}

#[derive(Debug, Default, Deserialize)]
struct OpenConnectionResponse {
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    ticket: String,
}

/// Registers a Stream connection and returns the dial-ready wss URL
/// (endpoint?ticket=…). `http` + `api_base` come from the shared outbound
/// [`crate::client::Client`] so tests can point them at a local server.
pub async fn open_connection(
    http: &reqwest::Client,
    api_base: &str,
    app_key: &str,
    app_secret: &str,
) -> anyhow::Result<String> {
    let body = serde_json::to_vec(&OpenConnectionRequest {
        client_id: app_key,
        client_secret: app_secret,
        subscriptions: chatbot_subscriptions(),
        ua: STREAM_USER_AGENT,
    })
    .map_err(|e| anyhow::anyhow!("marshal open request: {e}"))?;

    let url = format!(
        "{}{}",
        api_base.trim_end_matches('/'),
        CONNECTIONS_OPEN_PATH
    );
    let resp = http
        .post(url)
        .timeout(OPEN_CONNECT_TIMEOUT)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("open connection: {e}"))?;
    let status = resp.status();
    // Mirror Go's 1 MiB read cap on the response body.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("open connection: {e}"))?;
    let bytes = &bytes[..bytes.len().min(1 << 20)];
    if !status.is_success() {
        anyhow::bail!(
            "open connection: status {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(bytes)
        );
    }
    let out: OpenConnectionResponse =
        serde_json::from_slice(bytes).map_err(|e| anyhow::anyhow!("decode open response: {e}"))?;
    if out.endpoint.is_empty() || out.ticket.is_empty() {
        anyhow::bail!("open connection: empty endpoint or ticket");
    }
    let mut endpoint = url::Url::parse(&out.endpoint)
        .map_err(|_| anyhow::anyhow!("open connection: invalid secure websocket endpoint"))?;
    if endpoint.scheme() != "wss" || endpoint.host_str().unwrap_or("").is_empty() {
        anyhow::bail!("open connection: invalid secure websocket endpoint");
    }
    endpoint
        .query_pairs_mut()
        .append_pair("ticket", &out.ticket);
    Ok(endpoint.to_string())
}
