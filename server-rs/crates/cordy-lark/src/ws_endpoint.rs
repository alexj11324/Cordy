//! WS long-conn bootstrap.
//!
//! The bootstrap host for the long-conn `/callback/ws/endpoint` request is
//! the installation's open-platform host — open.feishu.cn for Feishu
//! (mainland), open.larksuite.com for Lark (international) — resolved per
//! call from InstallationCredentials.region via [`Region::open_platform_base_url`]
//! (Lark returns the actual wss URL in the response body, so only the
//! bootstrap POST host has to be region-aware). A deployment-wide
//! CORDY_LARK_CALLBACK_BASE_URL still overrides every installation when set
//! (staging / mock).

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::client::InstallationCredentials;
use crate::ws_connector::{EndpointFetcher, WsEndpoint};

/// HTTPConnectionTokenFetcher is the production EndpointFetcher. It exchanges
/// per-installation app credentials for a short-lived WebSocket URL +
/// ClientConfig by calling `POST /callback/ws/endpoint` on Lark's
/// open-platform host — the same bootstrap path the official
/// `larksuite/oapi-sdk-go/v3/ws` client uses. The request body carries
/// `{AppID, AppSecret}` plain (no tenant_access_token bearer); the response
/// carries the wss URL (single-use, embedded device_id/service_id auth) and a
/// ClientConfig with ping_interval / reconnect_interval / reconnect_nonce /
/// reconnect_count in seconds.
///
/// We do NOT cache the response. The wss URL is single-use by design (the
/// embedded `device_id` is rotated on every bootstrap call), so re-using it
/// on a reconnect would yield an auth rejection that looks like a Lark
/// outage. The connector calls endpoint() once per run.
///
/// PersonalAgent compatibility — OPEN RISK (PB-2671 review thread): the
/// official Feishu docs describe long-conn mode as "supports 企业自建应用
/// only". The PersonalAgent device-flow archetype is not listed as supported;
/// live confirmation is pending. If the bootstrap call returns a structured
/// "app type not supported" error, this code surfaces the code+msg directly
/// so the Hub's backoff loop logs the real reason instead of looping silently.
pub struct HttpConnectionTokenFetcher {
    cfg: HttpConnectionTokenConfig,
}

/// Wires the fetcher's dependencies. base_url is an optional deployment-wide
/// override; when empty (the production default) endpoint() resolves the
/// bootstrap host per installation from the region. Tests substitute a local
/// mock server URL to force all regions to the fake server.
#[derive(Clone, Default)]
pub struct HttpConnectionTokenConfig {
    pub base_url: String,
    pub http_client: Option<reqwest::Client>,
}

impl HttpConnectionTokenConfig {
    fn with_defaults(self) -> Self {
        // base_url is intentionally NOT defaulted here. Empty means "no
        // deployment-wide override" — endpoint() then resolves the bootstrap
        // host per installation from InstallationCredentials.region, so one
        // fetcher serves both Feishu and Lark.
        Self {
            base_url: self.base_url.trim_end_matches('/').to_string(),
            http_client: Some(self.http_client.unwrap_or_else(|| {
                reqwest::Client::builder()
                    .timeout(crate::http_client::DEFAULT_REQUEST_TIMEOUT)
                    .build()
                    .expect("reqwest client")
            })),
        }
    }
}

impl HttpConnectionTokenFetcher {
    /// Returns the production EndpointFetcher bound to the supplied
    /// configuration.
    pub fn new(cfg: HttpConnectionTokenConfig) -> Self {
        Self {
            cfg: cfg.with_defaults(),
        }
    }
}

/// bootstrapRequest mirrors the SDK's BootstrapRequest. Field names use
/// PascalCase exactly because the server-side JSON tags are PascalCase
/// (`AppID`, not `app_id`); the SDK's pbbp2 schema dictates the format and
/// lower-snake_case would not match.
#[derive(Deserialize)]
struct EndpointResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: EndpointData,
}

#[derive(Default, Deserialize)]
struct EndpointData {
    #[serde(rename = "URL", default)]
    url: String,
    #[serde(rename = "ClientConfig", default)]
    client_config: ClientConfig,
}

#[derive(Default, Deserialize)]
struct ClientConfig {
    #[serde(rename = "ReconnectCount", default)]
    reconnect_count: i32,
    #[serde(rename = "ReconnectInterval", default)]
    reconnect_interval: i64,
    #[serde(rename = "ReconnectNonce", default)]
    reconnect_nonce: i64,
    #[serde(rename = "PingInterval", default)]
    ping_interval: i64,
}

#[async_trait]
impl EndpointFetcher for HttpConnectionTokenFetcher {
    /// Implements EndpointFetcher.
    async fn endpoint(&self, creds: InstallationCredentials) -> anyhow::Result<WsEndpoint> {
        if creds.app_id.is_empty() || creds.app_secret.is_empty() {
            anyhow::bail!("lark ws endpoint: missing app_id / app_secret");
        }
        let body = serde_json::json!({
            "AppID": creds.app_id,
            "AppSecret": creds.app_secret,
        });
        // Resolve the bootstrap host per call: an explicit cfg.base_url
        // override wins (env / mock), otherwise the installation's region
        // picks Feishu vs Lark so one fetcher serves both clouds.
        let base = if self.cfg.base_url.is_empty() {
            creds.region.open_platform_base_url().to_string()
        } else {
            self.cfg.base_url.clone()
        };
        let url = format!("{base}/callback/ws/endpoint");
        let resp = self
            .cfg
            .http_client
            .as_ref()
            .expect("with_defaults fills http_client")
            .post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            // Locale header is sent verbatim by the SDK — Lark uses it for the
            // error `msg` field (Chinese vs English). We pick zh because
            // that's the audience Cordy server logs are read by today; if
            // i18n matters later this becomes an env or a per-installation
            // knob.
            .header("locale", "zh")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("http do: {e}"))?;
        let status = resp.status();
        let raw_resp = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("read body: {e}"))?;
        let raw_str = String::from_utf8_lossy(&raw_resp);
        if !status.is_success() {
            anyhow::bail!(
                "http {}: {}",
                status.as_u16(),
                crate::http_client::truncate(&raw_str, 512)
            );
        }
        let decoded: EndpointResponse = serde_json::from_slice(&raw_resp).map_err(|e| {
            anyhow::anyhow!(
                "decode response: {e} (raw={})",
                crate::http_client::truncate(&raw_str, 256)
            )
        })?;
        if decoded.code != 0 || decoded.data.url.is_empty() {
            // Surface the structured Lark error verbatim — that's what
            // operators need to disambiguate "app type not supported"
            // (PersonalAgent risk) from "credentials wrong" from "Lark
            // outage". The downstream Hub backoff logs this on each reconnect
            // attempt.
            anyhow::bail!(
                "lark ws endpoint: code={} msg={:?}",
                decoded.code,
                decoded.msg
            );
        }
        let service_id = parse_service_id_from_url(&decoded.data.url)
            .map_err(|e| anyhow::anyhow!("parse service_id from wss url: {e:#}"))?;
        Ok(WsEndpoint {
            url: decoded.data.url,
            headers: Vec::new(),
            service_id,
            ping_interval: Duration::from_secs(
                decoded.data.client_config.ping_interval.max(0) as u64
            ),
            reconnect_interval: Duration::from_secs(
                decoded.data.client_config.reconnect_interval.max(0) as u64,
            ),
            reconnect_nonce: Duration::from_secs(
                decoded.data.client_config.reconnect_nonce.max(0) as u64
            ),
            reconnect_count: decoded.data.client_config.reconnect_count,
        })
    }
}

/// Extracts the `service_id` query parameter Lark embeds in the wss URL. The
/// connector needs this value to address outbound Frame.service for
/// ping/pong and ACK frames; the SDK does the same.
pub fn parse_service_id_from_url(raw_url: &str) -> anyhow::Result<i32> {
    let parsed = url::Url::parse(raw_url)?;
    let sid = parsed
        .query_pairs()
        .find(|(k, _)| k == "service_id")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(|| anyhow::anyhow!("missing service_id query parameter"))?;
    let n: i64 = sid
        .parse()
        .map_err(|e| anyhow::anyhow!("service_id {sid:?} is not an int: {e}"))?;
    i32::try_from(n).map_err(|_| anyhow::anyhow!("service_id {sid:?} overflows int32"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_id_is_extracted_from_wss_query() {
        assert_eq!(
            parse_service_id_from_url(
                "wss://ws.open.feishu.cn/callback/ws/conn?device_id=dev&service_id=9"
            )
            .unwrap(),
            9
        );
        assert_eq!(
            parse_service_id_from_url("wss://host/path?service_id=1&other=x").unwrap(),
            1
        );
    }

    #[test]
    fn missing_or_bad_service_id_errors() {
        let err = parse_service_id_from_url("wss://host/path").unwrap_err();
        assert!(err.to_string().contains("missing service_id"));
        let err = parse_service_id_from_url("wss://host/path?service_id=abc").unwrap_err();
        assert!(err.to_string().contains("is not an int"));
        let err = parse_service_id_from_url("wss://host/path?service_id=99999999999").unwrap_err();
        assert!(err.to_string().contains("overflows int32"));
    }

    #[test]
    fn endpoint_response_decodes_pascal_case_wire_shape() {
        let raw = br#"{"code":0,"msg":"","data":{"URL":"wss://h/c?service_id=9","ClientConfig":{"ReconnectCount":3,"ReconnectInterval":60,"ReconnectNonce":5,"PingInterval":120}}}"#;
        let decoded: EndpointResponse = serde_json::from_slice(raw).unwrap();
        assert_eq!(decoded.code, 0);
        assert_eq!(decoded.data.url, "wss://h/c?service_id=9");
        assert_eq!(decoded.data.client_config.ping_interval, 120);
        assert_eq!(decoded.data.client_config.reconnect_count, 3);
    }

    #[test]
    fn config_strips_trailing_slash_and_defaults_client() {
        let cfg = HttpConnectionTokenConfig {
            base_url: "http://localhost:1234/".into(),
            http_client: None,
        };
        let fetcher = HttpConnectionTokenFetcher::new(cfg);
        assert_eq!(fetcher.cfg.base_url, "http://localhost:1234");
        assert!(fetcher.cfg.http_client.is_some());
    }

    #[test]
    fn region_resolves_bootstrap_host_when_no_override() {
        // Mirrors the Go contract: empty BaseURL defers to the region.
        assert_eq!(
            crate::types::Region::Lark.open_platform_base_url(),
            "https://open.larksuite.com"
        );
    }
}
