//! Real Lark/飞书 Open Platform HTTP ApiClient — port of
//! `server/internal/integrations/lark/http_client.go`.
//!
//! Scope: tenant_access_token acquisition + caching, IM v1 interactive-card
//! send / patch, the dedicated binding-prompt outbound, AND the install-time
//! Bot identity lookup (/open-apis/bot/v3/info) consumed by
//! RegistrationService right after a successful device-flow grant. The
//! PersonalAgent registration protocol itself is a separate client
//! ([`crate::registration`]) because it speaks to a different host
//! (accounts.feishu.cn) with a different auth model (no tenant_access_token —
//! the response IS the credentials).
//!
//! Per-installation credentials flow in on each call via
//! [`InstallationCredentials`]; the client never reads the installation table
//! directly. tenant_access_token is cached in-process keyed by app_id,
//! honoring Lark's `expire` field minus a safety margin so callers never
//! present a token that's about to lapse mid-flight.
//!
//! Port note: Go's per-client http.Client timeouts become per-request
//! timeouts on reqwest; context cancellation is enforced by callers wrapping
//! calls in tokio timeouts / cancellation selects (dropping the future aborts
//! the request).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::client::{
    AddReactionParams, ApiClient, ApiClientNotConfigured, BindingPromptParams, BotInfo,
    DeleteReactionParams, DownloadResourceParams, DownloadedResource, DownloadedResourceStream,
    InstallationCredentials, LarkMessage, LarkMessageMention, ListMessagesParams, PatchCardParams,
    ReplyTarget, SendCardParams, SendMarkdownCardParams, SendTextParams,
};
use crate::types::{ChatId, OpenId};

/// The default cap on one message-resource download. Exported so the
/// channel-media settle invariant test can assert the reconciler's settle
/// delay dwarfs every pipeline budget.
pub const DEFAULT_RESOURCE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45);

/// The mainland 飞书 open-platform host. It is the fallback host for an
/// installation whose region is feishu (or unset);
/// [`Region::open_platform_base_url`] maps region=lark to open.larksuite.com.
/// Operators do NOT set CORDY_LARK_HTTP_BASE_URL to pick a cloud anymore —
/// the per-installation region does that automatically. The env var remains
/// only as a deployment-wide override (proxy / mock / single-cloud staging).
pub const DEFAULT_LARK_BASE_URL: &str = "https://open.feishu.cn";

/// Subtracted from Lark's `expire` so we refresh before a token actually
/// lapses. 60s comfortably exceeds any in-flight HTTP timeout we set below.
const TOKEN_SAFETY_MARGIN: Duration = Duration::from_secs(60);

/// The per-call HTTP timeout. Lark's API is normally well under 1s; we leave
/// headroom for cross-region latency from a self-hosted Cordy deployment to
/// feishu.cn.
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Feishu caps message resources at 100 MiB. Keep the local transport guard
/// aligned with that contract; detached media processing keeps large
/// transfers off the connector ACK path.
const MAX_MESSAGE_RESOURCE_BYTES: usize = 100 << 20;

/// Lark's hard cap on a single im/v1/messages page. We clamp to it so a
/// caller asking for more silently gets the max rather than a 400 from Lark.
const LARK_LIST_MESSAGES_MAX_PAGE_SIZE: i32 = 50;

/// Lark's hard cap on user_ids per contact/v3/users/batch call. We drop the
/// overflow rather than error so a caller asking for more still gets the
/// first 50 resolved.
const LARK_BATCH_GET_USERS_MAX_IDS: usize = 50;

/// Lark's "invalid tenant_access_token" / "tenant_access_token expired"
/// error codes. When we see either, drop the cached token so the next call
/// refreshes from /tenant_access_token/internal. 99991663 = expired,
/// 99991664 = invalid. Documented at:
/// open.feishu.cn/document/server-docs/api-call-guide/server-error-codes
const CODE_TOKEN_EXPIRED: i64 = 99991663;
const CODE_TOKEN_INVALID: i64 = 99991664;

pub(crate) fn is_token_error(code: i64) -> bool {
    code == CODE_TOKEN_EXPIRED || code == CODE_TOKEN_INVALID
}

/// A structured Lark business error: the request reached Lark, returned HTTP
/// 200, but Lark rejected it with a non-zero `code`. This is distinct from
/// the transport-level errors do_json surfaces (network failure, 5xx,
/// timeout), which are returned as plain wrapped errors. The distinction
/// matters for the threaded-reply fallback: a business code is definitive
/// ("nothing was sent, and here is exactly why"), whereas a transport error
/// is ambiguous ("the message may or may not have been delivered") and must
/// NOT trigger a chat-level retry that could duplicate or leak the reply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("lark http client: {op}: code={code} msg={msg:?}")]
pub struct ApiError {
    pub op: String,
    pub code: i64,
    pub msg: String,
}

impl ApiError {
    pub(crate) fn new(op: &str, code: i64, msg: impl Into<String>) -> Self {
        Self {
            op: op.to_string(),
            code,
            msg: msg.into(),
        }
    }
}

/// Returns true when err is a Lark [`ApiError`] whose code means the threaded
/// reply cannot land on this target. Only such errors are safe to retry at
/// the chat level. Transport errors and other business codes return false.
///
/// threadReplyUnsupportedCodes are the reply-endpoint business codes that
/// definitively mean "this specific trigger message / topic cannot receive a
/// threaded reply" AND nothing was sent, while a plain chat-level send to the
/// same chat is unaffected. Only these justify the chat-level fallback. Rate
/// limits (230020), "message is being sent" (230049, ambiguous),
/// permission/content errors (which would also fail at chat level), and all
/// transport/5xx/timeout failures are deliberately excluded: those stay
/// failures so we never duplicate a reply or leak a thread-only reply into
/// the main group chat. Codes are from the IM reply-message endpoint error
/// table.
pub fn is_thread_reply_unsupported(err: &anyhow::Error) -> bool {
    const THREAD_REPLY_UNSUPPORTED_CODES: &[i64] =
        &[230011, 230019, 230050, 230071, 230072, 230111];
    err.chain().any(|cause| {
        cause
            .downcast_ref::<ApiError>()
            .is_some_and(|api| THREAD_REPLY_UNSUPPORTED_CODES.contains(&api.code))
    })
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    // Slice on a char boundary so multi-byte UTF-8 is not corrupted.
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Configures the production Lark HTTP ApiClient.
#[derive(Clone, Default)]
pub struct HttpClientConfig {
    /// Optional deployment-wide override for the Lark open-platform root,
    /// e.g. "https://open.feishu.cn" or "https://open.larksuite.com". When
    /// set it forces every call — regardless of the installation's region —
    /// to that host; tests set it to a local mock server URL. When EMPTY (the
    /// production default), each call resolves its host from
    /// InstallationCredentials.region so a single deployment serves both
    /// Feishu and Lark. Trailing "/" is stripped.
    pub base_url: String,

    /// The reqwest transport used for every outbound call. Tests substitute
    /// a client routed at a local mock server. Empty defaults to a fresh
    /// client with DEFAULT_REQUEST_TIMEOUT.
    pub http_client: Option<reqwest::Client>,

    /// Used only for message resource downloads. It deliberately does not
    /// share the main client's shorter timeout: image/video resource
    /// transfers are bounded by resource_download_timeout instead.
    pub resource_http_client: Option<reqwest::Client>,

    /// Caps a single message resource download. Zero defaults to
    /// DEFAULT_RESOURCE_DOWNLOAD_TIMEOUT.
    pub resource_download_timeout: Option<Duration>,

    /// Overridable for deterministic token-expiry tests.
    pub now: Option<fn() -> DateTime<Utc>>,
}

impl HttpClientConfig {
    fn with_defaults(self) -> Self {
        // base_url is intentionally NOT defaulted to DEFAULT_LARK_BASE_URL
        // here. An empty base_url means "no deployment-wide override" — each
        // call then resolves its host from InstallationCredentials.region
        // (see resolve_base_url), so one client serves both Feishu and Lark.
        // A non-empty base_url (CORDY_LARK_HTTP_BASE_URL, or a mock URL in
        // tests) forces every region to that host.
        let base_url = self.base_url.trim_end_matches('/').to_string();
        let resource_download_timeout = self
            .resource_download_timeout
            .unwrap_or(DEFAULT_RESOURCE_DOWNLOAD_TIMEOUT);
        let http_client = self.http_client.unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(DEFAULT_REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client builds with default settings")
        });
        let resource_http_client = self.resource_http_client.unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(resource_download_timeout)
                .build()
                .expect("reqwest client builds with default settings")
        });
        Self {
            base_url,
            http_client: Some(http_client),
            resource_download_timeout: Some(resource_download_timeout),
            resource_http_client: Some(resource_http_client),
            now: Some(self.now.unwrap_or(Utc::now)),
        }
    }
}

struct CachedToken {
    value: String,
    expires_at: DateTime<Utc>,
}

/// Constructs the real ApiClient that speaks to Lark's open platform over
/// HTTPS. Per-installation credentials flow in via each call's
/// InstallationCredentials parameter; tokens are cached keyed by app_id so a
/// single Cordy server reuses Lark's tenant_access_token across calls to the
/// same app.
pub struct HttpApiClient {
    cfg: HttpClientConfig,

    /// Caches tenant_access_token keyed by app_id only — NOT by
    /// (app_id, region). This is safe because a Lark/飞书 app_id (the
    /// "cli_..." credential) is globally unique across both clouds and an app
    /// exists on exactly one of them, so an app_id never maps to two regions.
    /// The DB enforces the same assumption with UNIQUE(app_id) on the
    /// installation config. If Lark ever reused an app_id across clouds, both
    /// this cache key and that constraint would need region added.
    tokens: Mutex<HashMap<String, CachedToken>>,
}

impl HttpApiClient {
    pub fn new(cfg: HttpClientConfig) -> Self {
        let cfg = cfg.with_defaults();
        Self {
            cfg,
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Picks the open-platform host for one call. An explicit cfg.base_url
    /// (CORDY_LARK_HTTP_BASE_URL, or a mock URL in tests) overrides every
    /// region and routes all traffic there. With no override, the host comes
    /// from the installation's region, so Feishu and Lark installations
    /// served by the same process each reach their own cloud.
    fn resolve_base_url(&self, creds: &InstallationCredentials) -> String {
        if !self.cfg.base_url.is_empty() {
            return self.cfg.base_url.clone();
        }
        creds.region.open_platform_base_url().to_string()
    }

    /// Drops the cached token for an app_id. Called when Lark surfaces an
    /// expired / invalid token error code so the next call refreshes instead
    /// of looping on a stale entry.
    fn invalidate_token(&self, app_id: &str) {
        self.tokens.lock().unwrap().remove(app_id);
    }

    /// Returns a usable tenant_access_token for the given installation,
    /// reusing a cached token while it is alive (minus safety margin) and
    /// otherwise fetching a fresh one from Lark.
    ///
    /// Concurrent callers serialize on the per-client mutex during the
    /// uncached path; the cached path takes the mutex only for the lookup
    /// and releases before doing any I/O. Steady-state contention is
    /// therefore one map-read under the lock, not a per-call HTTP round trip.
    async fn tenant_access_token(&self, creds: &InstallationCredentials) -> anyhow::Result<String> {
        if creds.app_id.is_empty() {
            anyhow::bail!("lark http client: missing app_id");
        }
        if creds.app_secret.is_empty() {
            anyhow::bail!("lark http client: missing app_secret");
        }

        let now = (self.cfg.now.unwrap_or(Utc::now))();
        if let Some(t) = self.tokens.lock().unwrap().get(&creds.app_id) {
            if t.expires_at > now {
                return Ok(t.value.clone());
            }
        }

        // Self-built (internal) app endpoint. Marketplace / multi-tenant apps
        // would use /tenant_access_token/v3 with a different body shape;
        // PersonalAgent in this MVP is per-workspace self-built so we stay on
        // /internal.
        #[derive(Deserialize)]
        struct TokenResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(rename = "tenant_access_token", default)]
            tenant_access_token: String,
            #[serde(default)]
            expire: i64,
        }
        let body = json!({
            "app_id": creds.app_id,
            "app_secret": creds.app_secret,
        });
        let resp: TokenResp = self
            .do_json(
                &self.resolve_base_url(creds),
                reqwest::Method::POST,
                "/open-apis/auth/v3/tenant_access_token/internal",
                "",
                Some(body),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: tenant_access_token: {e:#}"))?;
        if resp.code != 0 || resp.tenant_access_token.is_empty() {
            anyhow::bail!(
                "lark http client: tenant_access_token: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }

        let mut expire = Duration::from_secs(resp.expire.max(0) as u64);
        // Clamp to >= 2× safety margin so a misbehaving upstream that returns
        // a sub-minute expire never makes us cache a token that is already
        // past its safe window.
        if expire < TOKEN_SAFETY_MARGIN * 2 {
            expire = TOKEN_SAFETY_MARGIN * 2;
        }
        let expires_at = (self.cfg.now.unwrap_or(Utc::now))()
            + chrono::Duration::from_std(expire - TOKEN_SAFETY_MARGIN)
                .unwrap_or_else(|_| chrono::Duration::seconds(0));

        self.tokens.lock().unwrap().insert(
            creds.app_id.clone(),
            CachedToken {
                value: resp.tenant_access_token.clone(),
                expires_at,
            },
        );

        Ok(resp.tenant_access_token)
    }

    /// Encapsulates the verb + URL + auth-header + JSON encode/decode dance
    /// so each public method stays a thin shape-only adapter. base_url is the
    /// per-call open-platform host the caller resolved via resolve_base_url
    /// (region-aware). token == "" skips the Authorization header (only the
    /// tenant_access_token endpoint takes that path).
    async fn do_json<T: serde::de::DeserializeOwned>(
        &self,
        base_url: &str,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> anyhow::Result<T> {
        let url = format!("{base_url}{path}");
        let mut req = self
            .cfg
            .http_client
            .as_ref()
            .expect("with_defaults fills http_client")
            .request(method, &url);
        if let Some(body) = body {
            req = req
                .header("Content-Type", "application/json; charset=utf-8")
                .body(body.to_string());
        }
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("http do: {e}"))?;
        let status = resp.status();
        let raw_body = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("read body: {e}"))?;
        if !status.is_success() {
            anyhow::bail!(
                "http {}: {}",
                status.as_u16(),
                truncate(&raw_body_string(&raw_body), 512)
            );
        }
        if raw_body.is_empty() {
            // Mirror Go: an absent body decodes into zero values.
            return serde_json::from_str("{}")
                .map_err(|e| anyhow::anyhow!("decode empty body: {e}"));
        }
        serde_json::from_slice(&raw_body).map_err(|e| {
            anyhow::anyhow!(
                "decode body: {e} (raw={})",
                truncate(&raw_body_string(&raw_body), 256)
            )
        })
    }

    /// Builds the (path, body) the three send methods share. When target is
    /// set the message is routed through Lark's reply endpoint
    /// (POST /im/v1/messages/{message_id}/reply) so it threads back into the
    /// originating 话题 — reply_in_thread carries the target's in_thread flag
    /// (Lark also keeps the reply in-thread automatically when the parent
    /// message already belongs to a thread). Otherwise the message goes to
    /// the chat-level send endpoint keyed by receive_id=chat_id, the
    /// historical behavior.
    fn outbound_message_request(
        chat_id: &ChatId,
        msg_type: &str,
        content: &str,
        target: &ReplyTarget,
    ) -> (String, serde_json::Value) {
        if target.is_set() {
            return (
                format!(
                    "/open-apis/im/v1/messages/{}/reply",
                    url_path_escape(&target.message_id)
                ),
                json!({
                    "msg_type": msg_type,
                    "content": content,
                    "reply_in_thread": target.in_thread,
                }),
            );
        }
        (
            "/open-apis/im/v1/messages?receive_id_type=chat_id".to_string(),
            json!({
                "receive_id": chat_id.0,
                "msg_type": msg_type,
                "content": content,
            }),
        )
    }

    async fn fetch_bot_union_id(
        &self,
        base_url: &str,
        app_id: &str,
        token: &str,
        open_id: &str,
    ) -> anyhow::Result<String> {
        if open_id.is_empty() {
            anyhow::bail!("empty open_id");
        }
        #[derive(Deserialize)]
        struct ContactResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: ContactData,
        }
        #[derive(Default, Deserialize)]
        struct ContactData {
            #[serde(default)]
            user: ContactUser,
        }
        #[derive(Default, Deserialize)]
        struct ContactUser {
            #[serde(rename = "union_id", default)]
            union_id: String,
        }
        let path = format!(
            "/open-apis/contact/v3/users/{}?user_id_type=open_id",
            url_path_escape(open_id)
        );
        let resp: ContactResp = self
            .do_json(base_url, reqwest::Method::GET, &path, token, None)
            .await
            .map_err(|e| anyhow::anyhow!("contact users: {e:#}"))?;
        if resp.code != 0 {
            // invalidate_token is keyed by app_id (the cache key on
            // HttpApiClient.tokens), NOT by the bearer string. Passing the
            // bearer would do nothing and a stale token would keep being
            // reused on every retry until natural TTL expiry.
            if is_token_error(resp.code) {
                self.invalidate_token(app_id);
            }
            anyhow::bail!("contact users: code={} msg={:?}", resp.code, resp.msg);
        }
        Ok(resp.data.user.union_id)
    }
}

fn raw_body_string(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Percent-encodes a single URL path segment (Go's url.PathEscape).
fn url_path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[async_trait]
impl ApiClient for HttpApiClient {
    /// Reports true: once this client exists at all, the outbound transport
    /// path (send / patch / binding prompt / bot info) is wired. The stub
    /// returns false because every call there errors with
    /// ApiClientNotConfigured; the real client is the inverse contract.
    fn is_configured(&self) -> bool {
        true
    }

    /// Posts a fresh interactive card into a chat and returns Lark's
    /// message_id so the Patcher can target subsequent patches at the same
    /// card.
    async fn send_interactive_card(&self, p: SendCardParams) -> anyhow::Result<String> {
        if p.chat_id.is_empty() {
            anyhow::bail!("lark http client: missing chat_id");
        }
        if p.card_json.is_empty() {
            anyhow::bail!("lark http client: missing card json");
        }
        let token = self.tenant_access_token(&p.installation_id).await?;
        let (path, body) = Self::outbound_message_request(
            &p.chat_id,
            "interactive",
            &p.card_json,
            &p.reply_target,
        );
        #[derive(Deserialize)]
        struct SendResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: SendData,
        }
        #[derive(Default, Deserialize)]
        struct SendData {
            #[serde(rename = "message_id", default)]
            message_id: String,
        }
        let resp: SendResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::POST,
                &path,
                &token,
                Some(body),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: send interactive card: {e:#}"))?;
        if resp.code != 0 || resp.data.message_id.is_empty() {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            return Err(ApiError::new("send interactive card", resp.code, resp.msg).into());
        }
        Ok(resp.data.message_id)
    }

    /// Posts a plain text IM message into a Lark chat. This is the Patcher's
    /// primary outbound for agent chat replies — using a normal text bubble
    /// instead of an interactive card makes free-form replies feel like a
    /// native Lark conversation. The content envelope Lark expects is a
    /// JSON-encoded `{"text": "..."}` blob; we encode it here so callers pass
    /// raw text.
    async fn send_text_message(&self, p: SendTextParams) -> anyhow::Result<String> {
        if p.chat_id.is_empty() {
            anyhow::bail!("lark http client: missing chat_id");
        }
        if p.text.is_empty() {
            anyhow::bail!("lark http client: missing text");
        }
        let token = self.tenant_access_token(&p.installation_id).await?;
        // Lark's `text` msg_type expects content = JSON-encoded {"text":
        // "..."}; serde handles the escape of newlines / quotes / unicode so
        // the agent's reply round-trips intact.
        let content = json!({"text": p.text}).to_string();
        let (path, body) =
            Self::outbound_message_request(&p.chat_id, "text", &content, &p.reply_target);
        #[derive(Deserialize)]
        struct SendResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: SendData,
        }
        #[derive(Default, Deserialize)]
        struct SendData {
            #[serde(rename = "message_id", default)]
            message_id: String,
        }
        let resp: SendResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::POST,
                &path,
                &token,
                Some(body),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: send text message: {e:#}"))?;
        if resp.code != 0 || resp.data.message_id.is_empty() {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            return Err(ApiError::new("send text message", resp.code, resp.msg).into());
        }
        Ok(resp.data.message_id)
    }

    /// Posts the agent's reply as an interactive card using Lark's schema-2.0
    /// envelope with a single `tag: "markdown"` body element. Lark's client
    /// renders the markdown into formatted text (bold, italics, lists, links,
    /// fenced code blocks, tables, …) rather than showing raw markdown
    /// characters as it does for `msg_type=text`. We deliberately keep
    /// send_text_message as a separate path for plain-prose replies — a card
    /// around a one-line "Hello!" adds visual chrome that the user doesn't
    /// want; the routing decision (markdown vs text) lives at the Patcher
    /// layer.
    ///
    /// Why schema 2.0 rather than the legacy schema with a `div` + `lark_md`
    /// text element: the legacy `lark_md` tag's markdown dialect is much
    /// narrower — no fenced code blocks (syntax highlighting), no tables, no
    /// heading sizes. Schema-2.0's `markdown` tag is closer to GFM.
    async fn send_markdown_card(&self, p: SendMarkdownCardParams) -> anyhow::Result<String> {
        if p.chat_id.is_empty() {
            anyhow::bail!("lark http client: missing chat_id");
        }
        if p.markdown.is_empty() {
            anyhow::bail!("lark http client: missing markdown body");
        }
        let token = self.tenant_access_token(&p.installation_id).await?;
        let mut card = json!({
            "schema": "2.0",
            "body": {
                "elements": [
                    {"tag": "markdown", "content": p.markdown},
                ],
            },
        });
        if !p.summary.is_empty() {
            card["config"] = json!({"summary": {"content": p.summary}});
        }
        let (path, body) = Self::outbound_message_request(
            &p.chat_id,
            "interactive",
            &card.to_string(),
            &p.reply_target,
        );
        #[derive(Deserialize)]
        struct SendResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: SendData,
        }
        #[derive(Default, Deserialize)]
        struct SendData {
            #[serde(rename = "message_id", default)]
            message_id: String,
        }
        let resp: SendResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::POST,
                &path,
                &token,
                Some(body),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: send markdown card: {e:#}"))?;
        if resp.code != 0 || resp.data.message_id.is_empty() {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            return Err(ApiError::new("send markdown card", resp.code, resp.msg).into());
        }
        Ok(resp.data.message_id)
    }

    /// Updates an existing card's body. Lark's message-patch endpoint
    /// replaces the whole card payload; callers (i.e. the Patcher) render the
    /// full updated card each time.
    async fn patch_interactive_card(&self, p: PatchCardParams) -> anyhow::Result<()> {
        if p.lark_card_message_id.is_empty() {
            anyhow::bail!("lark http client: missing card message id");
        }
        if p.card_json.is_empty() {
            anyhow::bail!("lark http client: missing card json");
        }
        let token = self.tenant_access_token(&p.installation_id).await?;
        #[derive(Deserialize)]
        struct PatchResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
        }
        let path = format!(
            "/open-apis/im/v1/messages/{}",
            url_path_escape(&p.lark_card_message_id)
        );
        let resp: PatchResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::PATCH,
                &path,
                &token,
                Some(json!({"content": p.card_json})),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: patch interactive card: {e:#}"))?;
        if resp.code != 0 {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            anyhow::bail!(
                "lark http client: patch interactive card: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        Ok(())
    }

    /// Renders the member-binding card and posts it directly to the unbound
    /// user's open_id (not the chat). Keeping the card template inside this
    /// client — rather than the dispatcher — means the dispatcher never has
    /// to know about Lark's card schema.
    async fn send_binding_prompt_card(&self, p: BindingPromptParams) -> anyhow::Result<()> {
        if p.open_id.is_empty() {
            anyhow::bail!("lark http client: missing open_id");
        }
        if p.bind_url.is_empty() {
            anyhow::bail!("lark http client: missing bind url");
        }
        let card_json = binding_prompt_template(&p.bind_url)?;
        let token = self.tenant_access_token(&p.installation_id).await?;
        #[derive(Deserialize)]
        struct PromptResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
        }
        let path = "/open-apis/im/v1/messages?receive_id_type=open_id";
        let resp: PromptResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::POST,
                path,
                &token,
                Some(json!({
                    "receive_id": p.open_id.0,
                    "msg_type": "interactive",
                    "content": card_json,
                })),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: send binding prompt: {e:#}"))?;
        if resp.code != 0 {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            anyhow::bail!(
                "lark http client: send binding prompt: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        Ok(())
    }

    /// Calls /open-apis/bot/v3/info to learn the Bot's per-installation
    /// open_id and then /open-apis/contact/v3/users/{open_id}?user_id_type=
    /// open_id to resolve its stable union_id. RegistrationService is the
    /// only caller — right after the device-flow registration returns fresh
    /// client_id / client_secret, the service mints a tenant_access_token
    /// with those creds and calls this method so the installation row can be
    /// frozen with both Bot identifiers in the same transaction as the
    /// installer-bind.
    ///
    /// Why two API calls instead of one: /bot/v3/info does not return
    /// union_id in the public schema. The WS inbound decoder needs union_id
    /// to disambiguate which bot was @-mentioned in a multi-bot group chat
    /// (the per-app open_id field on mentions is structurally inverse across
    /// WS perspectives — see MUL-2671 triage), so we invest one extra HTTP
    /// round-trip at install time to capture it and avoid running the wrong
    /// supervisor for every event going forward.
    ///
    /// A missing union_id (contact lookup denied by app scope, or Lark
    /// returns an empty field) is NOT a hard failure here — the installation
    /// is still usable for p2p chats and the decoder can fall back to the
    /// (broken) open_id match path until the operator fixes scopes. We log a
    /// warning so the gap is visible.
    async fn get_bot_info(&self, creds: InstallationCredentials) -> anyhow::Result<BotInfo> {
        if creds.app_id.is_empty() || creds.app_secret.is_empty() {
            anyhow::bail!("lark http client: missing app credentials for get_bot_info");
        }
        let token = self.tenant_access_token(&creds).await?;
        #[derive(Deserialize)]
        struct BotResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            bot: BotData,
        }
        #[derive(Default, Deserialize)]
        struct BotData {
            #[serde(rename = "open_id", default)]
            open_id: String,
        }
        let bot_resp: BotResp = self
            .do_json(
                &self.resolve_base_url(&creds),
                reqwest::Method::GET,
                "/open-apis/bot/v3/info",
                &token,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: bot info: {e:#}"))?;
        if bot_resp.code != 0 {
            if is_token_error(bot_resp.code) {
                self.invalidate_token(&creds.app_id);
            }
            anyhow::bail!(
                "lark http client: bot info: code={} msg={:?}",
                bot_resp.code,
                bot_resp.msg
            );
        }
        if bot_resp.bot.open_id.is_empty() {
            anyhow::bail!("lark http client: bot info: response missing open_id");
        }

        // Resolve union_id via the contact endpoint. Soft-fail: log and
        // return the BotInfo with empty union_id. Callers
        // (RegistrationService.finish_success) accept the gap and persist
        // what they have.
        let union_id = match self
            .fetch_bot_union_id(
                &self.resolve_base_url(&creds),
                &creds.app_id,
                &token,
                &bot_resp.bot.open_id,
            )
            .await
        {
            Ok(u) => u,
            Err(lookup_err) => {
                tracing::warn!(
                    app_id = %creds.app_id,
                    bot_open_id = %bot_resp.bot.open_id,
                    error = %lookup_err,
                    "lark http client: bot union_id lookup failed; continuing without it"
                );
                String::new()
            }
        };
        Ok(BotInfo {
            open_id: OpenId(bot_resp.bot.open_id),
            union_id,
        })
    }

    /// Retrieves a message by id via GET /open-apis/im/v1/messages/
    /// {message_id}. The endpoint always wraps the result in data.items[] —
    /// one element for a normal message, and a forward sentinel followed by
    /// the bundled child messages for a merge_forward. We pass
    /// user_id_type=open_id so sender.id and mentions[].id come back as
    /// open_ids, matching the identifiers the rest of the crate keys on.
    ///
    /// body.content is forwarded verbatim (the raw, JSON-encoded, msg_type-
    /// specific string Lark double-encodes); the enricher's flattener owns
    /// interpreting it. A deleted / out-of-scope message surfaces as a Lark
    /// error code, which we turn into a normal error so the enricher can
    /// degrade to its "[unable to fetch]" placeholder without aborting the
    /// inbound pipeline.
    async fn get_message(
        &self,
        creds: InstallationCredentials,
        message_id: &str,
    ) -> anyhow::Result<Vec<LarkMessage>> {
        if message_id.is_empty() {
            anyhow::bail!("lark http client: missing message_id");
        }
        let token = self.tenant_access_token(&creds).await?;
        let path = format!(
            "/open-apis/im/v1/messages/{}?user_id_type=open_id",
            url_path_escape(message_id)
        );
        let resp: ItemsResp = self
            .do_json(
                &self.resolve_base_url(&creds),
                reqwest::Method::GET,
                &path,
                &token,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: get message: {e:#}"))?;
        if resp.code != 0 {
            if is_token_error(resp.code) {
                self.invalidate_token(&creds.app_id);
            }
            anyhow::bail!(
                "lark http client: get message: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        Ok(resp.data.items.iter().map(|it| it.normalize()).collect())
    }

    /// Retrieves a bounded, recent window of messages via
    /// GET /open-apis/im/v1/messages. Where get_message fetches a single
    /// message by id, this lists a conversation; it backs the enricher's
    /// group-context prefetch. The container is chat (container_id_type=
    /// chat) by default, or a single Lark topic (container_id_type=thread)
    /// when p.thread_id is set — the thread container keeps a topic
    /// @-mention from seeing sibling topics that share the chat_id (#5835).
    /// We pass sort_type=ByCreateTimeDesc so the newest messages come first
    /// and a small page_size captures "the last N" without paginating,
    /// keeping the inbound ACK path's fan-out to a single round-trip.
    /// user_id_type=open_id matches the identifiers the rest of the crate
    /// keys on; body.content is forwarded verbatim for the enricher's
    /// flattener to interpret.
    async fn list_chat_messages(
        &self,
        creds: InstallationCredentials,
        p: ListMessagesParams,
    ) -> anyhow::Result<Vec<LarkMessage>> {
        if p.chat_id.is_empty() {
            anyhow::bail!("lark http client: missing chat_id");
        }
        let size = if p.page_size <= 0 {
            1
        } else if p.page_size > LARK_LIST_MESSAGES_MAX_PAGE_SIZE {
            LARK_LIST_MESSAGES_MAX_PAGE_SIZE
        } else {
            p.page_size
        };
        let token = self.tenant_access_token(&creds).await?;

        let mut q: Vec<(String, String)> = Vec::new();
        if !p.thread_id.is_empty() {
            // Topic-scoped window: only this 话题's messages, so a @-mention
            // inside a topic never pulls sibling topics that share the
            // chat_id (#5835). The thread container rejects end_time, so it
            // is omitted here; the caller anchors the window to the trigger
            // time client-side instead.
            q.push(("container_id_type".into(), "thread".into()));
            q.push(("container_id".into(), p.thread_id.clone()));
        } else {
            q.push(("container_id_type".into(), "chat".into()));
            q.push(("container_id".into(), p.chat_id.0.clone()));
            if p.end_time > 0 {
                q.push(("end_time".into(), p.end_time.to_string()));
            }
        }
        q.push(("sort_type".into(), "ByCreateTimeDesc".into()));
        q.push(("page_size".into(), size.to_string()));
        q.push(("user_id_type".into(), "open_id".into()));
        let path = format!("/open-apis/im/v1/messages?{}", urlencode_pairs(&q));

        let resp: ItemsResp = self
            .do_json(
                &self.resolve_base_url(&creds),
                reqwest::Method::GET,
                &path,
                &token,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: list chat messages: {e:#}"))?;
        if resp.code != 0 {
            if is_token_error(resp.code) {
                self.invalidate_token(&creds.app_id);
            }
            anyhow::bail!(
                "lark http client: list chat messages: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        Ok(resp.data.items.iter().map(|it| it.normalize()).collect())
    }

    /// Obtains a binary message resource (image, video, file, audio) from
    /// Lark/Feishu. Business errors are still represented as JSON with a
    /// code/msg body on some failures, so JSON-looking responses are checked
    /// before being treated as resource bytes.
    async fn download_message_resource(
        &self,
        creds: InstallationCredentials,
        p: DownloadResourceParams,
    ) -> anyhow::Result<DownloadedResource> {
        let stream = self.download_message_resource_stream(creds, p).await?;
        let content_type = stream.content_type.clone();
        let filename = stream.filename.clone();
        let reported_size = stream.size_bytes;
        let raw_body = stream
            .read_to_end()
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: download resource: read body: {e}"))?;
        let size_bytes = if reported_size == 0 {
            raw_body.len() as i64
        } else {
            reported_size
        };
        Ok(DownloadedResource {
            data: raw_body,
            content_type,
            filename,
            size_bytes,
        })
    }

    /// Adds an emoji reaction to a message via POST
    /// /open-apis/im/v1/messages/{message_id}/reactions. Returns the
    /// reaction_id so it can be deleted later.
    async fn add_message_reaction(&self, p: AddReactionParams) -> anyhow::Result<String> {
        if p.message_id.is_empty() {
            anyhow::bail!("lark http client: missing message_id");
        }
        if p.emoji_type.is_empty() {
            anyhow::bail!("lark http client: missing emoji_type");
        }
        let token = self.tenant_access_token(&p.installation_id).await?;
        #[derive(Deserialize)]
        struct ReactionResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: ReactionData,
        }
        #[derive(Default, Deserialize)]
        struct ReactionData {
            #[serde(rename = "reaction_id", default)]
            reaction_id: String,
        }
        let path = format!(
            "/open-apis/im/v1/messages/{}/reactions",
            url_path_escape(&p.message_id)
        );
        let resp: ReactionResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::POST,
                &path,
                &token,
                Some(json!({"reaction_type": {"emoji_type": p.emoji_type}})),
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: add message reaction: {e:#}"))?;
        if resp.code != 0 || resp.data.reaction_id.is_empty() {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            anyhow::bail!(
                "lark http client: add message reaction: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        Ok(resp.data.reaction_id)
    }

    /// Removes a reaction from a message via DELETE
    /// /open-apis/im/v1/messages/{message_id}/reactions/{reaction_id}.
    async fn delete_message_reaction(&self, p: DeleteReactionParams) -> anyhow::Result<()> {
        if p.message_id.is_empty() {
            anyhow::bail!("lark http client: missing message_id");
        }
        if p.reaction_id.is_empty() {
            anyhow::bail!("lark http client: missing reaction_id");
        }
        let token = self.tenant_access_token(&p.installation_id).await?;
        #[derive(Deserialize)]
        struct DelResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
        }
        let path = format!(
            "/open-apis/im/v1/messages/{}/reactions/{}",
            url_path_escape(&p.message_id),
            url_path_escape(&p.reaction_id)
        );
        let resp: DelResp = self
            .do_json(
                &self.resolve_base_url(&p.installation_id),
                reqwest::Method::DELETE,
                &path,
                &token,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: delete message reaction: {e:#}"))?;
        if resp.code != 0 {
            if is_token_error(resp.code) {
                self.invalidate_token(&p.installation_id.app_id);
            }
            anyhow::bail!(
                "lark http client: delete message reaction: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        Ok(())
    }

    /// Resolves user open_ids to display names via
    /// GET /open-apis/contact/v3/users/batch?user_ids=…&user_id_type=open_id.
    /// It mirrors fetch_bot_union_id's single-user contact lookup, batched.
    /// Only id→name pairs the API actually returns are included; a restricted
    /// contact scope or an unknown id simply yields a smaller map (code==0
    /// with fewer items), never an error, so the enricher degrades to
    /// positional speaker labels. Ids past Lark's 50-per-call cap are dropped.
    async fn batch_get_users(
        &self,
        creds: InstallationCredentials,
        open_ids: &[String],
    ) -> anyhow::Result<HashMap<String, String>> {
        if open_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<&String> = open_ids.iter().take(LARK_BATCH_GET_USERS_MAX_IDS).collect();
        let token = self.tenant_access_token(&creds).await?;
        let mut q: Vec<(String, String)> = vec![("user_id_type".into(), "open_id".into())];
        for id in ids {
            if !id.is_empty() {
                q.push(("user_ids".into(), id.clone()));
            }
        }
        let path = format!("/open-apis/contact/v3/users/batch?{}", urlencode_pairs(&q));

        #[derive(Deserialize)]
        struct BatchResp {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: BatchData,
        }
        #[derive(Default, Deserialize)]
        struct BatchData {
            #[serde(default)]
            items: Vec<BatchItem>,
        }
        #[derive(Default, Deserialize)]
        struct BatchItem {
            #[serde(rename = "open_id", default)]
            open_id: String,
            #[serde(default)]
            name: String,
        }
        let resp: BatchResp = self
            .do_json(
                &self.resolve_base_url(&creds),
                reqwest::Method::GET,
                &path,
                &token,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: batch get users: {e:#}"))?;
        if resp.code != 0 {
            if is_token_error(resp.code) {
                self.invalidate_token(&creds.app_id);
            }
            anyhow::bail!(
                "lark http client: batch get users: code={} msg={:?}",
                resp.code,
                resp.msg
            );
        }
        let mut out = HashMap::with_capacity(resp.data.items.len());
        for it in resp.data.items {
            if !it.open_id.is_empty() && !it.name.is_empty() {
                out.insert(it.open_id, it.name);
            }
        }
        Ok(out)
    }
}

impl HttpApiClient {
    /// Streaming variant of download_message_resource. The returned reader is
    /// bounded by MAX_MESSAGE_RESOURCE_BYTES; reading past the cap errors
    /// instead of growing without bound.
    pub async fn download_message_resource_stream(
        &self,
        creds: InstallationCredentials,
        p: DownloadResourceParams,
    ) -> anyhow::Result<DownloadedResourceStream> {
        use futures_util::StreamExt;
        use tokio_util::io::StreamReader;

        if p.message_id.is_empty() {
            anyhow::bail!("lark http client: missing message_id");
        }
        if p.file_key.is_empty() {
            anyhow::bail!("lark http client: missing file_key");
        }
        let token = self.tenant_access_token(&creds).await?;
        let mut q: Vec<(String, String)> = Vec::new();
        if !p.r#type.is_empty() {
            q.push(("type".into(), p.r#type.clone()));
        }
        let mut path = format!(
            "/open-apis/im/v1/messages/{}/resources/{}",
            url_path_escape(&p.message_id),
            url_path_escape(&p.file_key)
        );
        if !q.is_empty() {
            path.push('?');
            path.push_str(&urlencode_pairs(&q));
        }

        let url = format!("{}{path}", self.resolve_base_url(&creds));
        let resp = self
            .cfg
            .resource_http_client
            .as_ref()
            .expect("with_defaults fills resource_http_client")
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .timeout(
                self.cfg
                    .resource_download_timeout
                    .unwrap_or(DEFAULT_RESOURCE_DOWNLOAD_TIMEOUT),
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("lark http client: download resource: http do: {e}"))?;

        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let disposition = resp
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if resp
            .content_length()
            .is_some_and(|size| size > MAX_MESSAGE_RESOURCE_BYTES as u64)
        {
            anyhow::bail!(
                "lark http client: download resource: resource exceeds {} bytes",
                MAX_MESSAGE_RESOURCE_BYTES
            );
        }

        if !status.is_success() {
            let raw_body = read_bounded(resp, MAX_MESSAGE_RESOURCE_BYTES).await?;
            anyhow::bail!(
                "lark http client: download resource: http {}: {}",
                status.as_u16(),
                truncate(&raw_body_string(&raw_body), 512)
            );
        }

        if content_type.to_lowercase().contains("json") {
            let raw_body = read_bounded(resp, MAX_MESSAGE_RESOURCE_BYTES).await?;
            #[derive(Deserialize)]
            struct ApiErrBody {
                #[serde(default)]
                code: i64,
                #[serde(default)]
                msg: String,
            }
            if let Ok(api_resp) = serde_json::from_slice::<ApiErrBody>(&raw_body) {
                if api_resp.code != 0 {
                    if is_token_error(api_resp.code) {
                        self.invalidate_token(&creds.app_id);
                    }
                    return Err(
                        ApiError::new("download resource", api_resp.code, api_resp.msg).into(),
                    );
                }
            }
            let size = raw_body.len() as i64;
            return Ok(DownloadedResourceStream {
                body: Box::new(std::io::Cursor::new(raw_body)),
                content_type,
                filename: filename_from_content_disposition(&disposition),
                size_bytes: size,
            });
        }

        let content_type = if content_type.is_empty() {
            "application/octet-stream".to_string()
        } else {
            content_type
        };
        let mut read = 0usize;
        let byte_stream = resp.bytes_stream().map(move |result| {
            let chunk = result.map_err(std::io::Error::other)?;
            read = read.saturating_add(chunk.len());
            if read > MAX_MESSAGE_RESOURCE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "lark resource exceeds download limit",
                ));
            }
            Ok(chunk)
        });
        let reader = StreamReader::new(byte_stream);
        Ok(DownloadedResourceStream {
            body: Box::new(reader),
            content_type,
            filename: filename_from_content_disposition(&disposition),
            size_bytes: 0,
        })
    }
}

/// Reads up to cap+1 bytes so an over-cap body can be detected; errors when
/// the body exceeds cap (mirrors Go's readMessageResourceErrorBody).
async fn read_bounded(resp: reqwest::Response, cap: usize) -> anyhow::Result<Vec<u8>> {
    use futures_util::StreamExt;
    use tokio::io::AsyncReadExt as _;
    use tokio_util::io::StreamReader;

    let byte_stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let mut reader = StreamReader::new(byte_stream).take(cap as u64 + 1);
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf).await?;
    if buf.len() > cap {
        anyhow::bail!(
            "lark http client: download resource: resource exceeds {} bytes",
            cap
        );
    }
    Ok(buf)
}

/// Extracts `filename=` from a Content-Disposition header (Go:
/// mime.ParseMediaType params["filename"]).
pub(crate) fn filename_from_content_disposition(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("filename=") {
            let value = value.trim();
            return value.trim_matches('"').to_string();
        }
    }
    String::new()
}

/// application/x-www-form-urlencoded pair encoding (Go's url.Values.Encode):
/// keys sorted, values percent-encoded with space as '+'.
pub(crate) fn urlencode_pairs(q: &[(String, String)]) -> String {
    let mut encoded: Vec<(String, String)> = q
        .iter()
        .map(|(k, v)| (form_encode(k), form_encode(v)))
        .collect();
    encoded.sort();
    encoded
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'*' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// larkRESTMessageItem is the IM v1 message item shape returned by the get /
/// list endpoints. It differs from the WS receive event in two ways the
/// enricher cares about: msg_type (not message_type), and a flat sender.id /
/// mentions[].id string (not a nested id object).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RestMessageItem {
    message_id: String,
    root_id: String,
    parent_id: String,
    thread_id: String,
    upper_message_id: String,
    msg_type: String,
    create_time: String,
    deleted: bool,
    sender: RestSender,
    body: RestBody,
    mentions: Vec<RestMention>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RestSender {
    id: String,
    id_type: String,
    sender_type: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RestBody {
    content: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RestMention {
    key: String,
    id: String,
    name: String,
}

impl RestMessageItem {
    fn normalize(&self) -> LarkMessage {
        LarkMessage {
            message_id: self.message_id.clone(),
            message_type: self.msg_type.clone(),
            content: self.body.content.clone(),
            sender_id: self.sender.id.clone(),
            sender_type: self.sender.sender_type.clone(),
            create_time: self.create_time.clone(),
            parent_id: self.parent_id.clone(),
            root_id: self.root_id.clone(),
            thread_id: self.thread_id.clone(),
            upper_message_id: self.upper_message_id.clone(),
            deleted: self.deleted,
            mentions: self
                .mentions
                .iter()
                .map(|mn| LarkMessageMention {
                    key: mn.key.clone(),
                    id: mn.id.clone(),
                    name: mn.name.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ItemsResp {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: ItemsData,
}

#[derive(Debug, Default, Deserialize)]
struct ItemsData {
    #[serde(default)]
    items: Vec<RestMessageItem>,
}

/// Renders the "you need to bind" interactive card. Single primary CTA
/// pointing at the redemption URL; the rest of the body is plain-text Chinese
/// copy matching the in-app voice.
///
/// Kept here (not in the default renderer) so the binding card template can
/// evolve independently of the streaming-status cards the Patcher renders —
/// they have different lifecycles (binding card is one-shot, status cards are
/// patched in place).
pub(crate) fn binding_prompt_template(bind_url: &str) -> anyhow::Result<String> {
    let doc = json!({
        "config": {"wide_screen_mode": true},
        "header": {
            "template": "blue",
            "title": {"tag": "plain_text", "content": "Cordy"},
        },
        "elements": [
            {
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": "你还没有绑定 Cordy 账户。点击下方按钮完成绑定后即可使用此 Agent。",
                },
            },
            {
                "tag": "action",
                "actions": [
                    {
                        "tag": "button",
                        "text": {"tag": "plain_text", "content": "去绑定"},
                        "type": "primary",
                        "url": bind_url,
                    },
                ],
            },
        ],
    });
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Region;

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 4), "hell…");
        // Multi-byte characters are never split mid-rune.
        let cn = "你好世界";
        assert_eq!(truncate(cn, 5), "你…");
        assert_eq!(truncate(cn, 8), "你好…");
    }

    #[test]
    fn thread_reply_unsupported_codes_classify() {
        let err: anyhow::Error = ApiError::new("op", 230011, "recalled").into();
        assert!(is_thread_reply_unsupported(&err));
        let rate: anyhow::Error = ApiError::new("op", 230020, "rate limited").into();
        assert!(!is_thread_reply_unsupported(&rate));
        let transport: anyhow::Error = anyhow::anyhow!("http do: connection reset");
        assert!(!is_thread_reply_unsupported(&transport));
    }

    #[test]
    fn token_error_codes_match_lark_docs() {
        assert!(is_token_error(99991663));
        assert!(is_token_error(99991664));
        assert!(!is_token_error(0));
        assert!(!is_token_error(230011));
    }

    #[test]
    fn outbound_message_request_routes_reply_vs_chat_level() {
        let (path, body) = HttpApiClient::outbound_message_request(
            &ChatId("oc_chat".into()),
            "text",
            "{\"text\":\"hi\"}",
            &ReplyTarget {
                message_id: "om_1".into(),
                in_thread: true,
            },
        );
        assert_eq!(path, "/open-apis/im/v1/messages/om_1/reply");
        assert_eq!(body["reply_in_thread"], serde_json::json!(true));

        let (path, body) = HttpApiClient::outbound_message_request(
            &ChatId("oc_chat".into()),
            "text",
            "{\"text\":\"hi\"}",
            &ReplyTarget::default(),
        );
        assert_eq!(path, "/open-apis/im/v1/messages?receive_id_type=chat_id");
        assert_eq!(body["receive_id"], serde_json::json!("oc_chat"));
    }

    #[test]
    fn rest_item_normalizes_to_lark_message() {
        let item: RestMessageItem = serde_json::from_value(serde_json::json!({
            "message_id": "om_1",
            "root_id": "om_root",
            "parent_id": "om_parent",
            "thread_id": "omt_1",
            "upper_message_id": "",
            "msg_type": "text",
            "create_time": "1700000000000",
            "deleted": false,
            "sender": {"id": "ou_1", "id_type": "open_id", "sender_type": "user"},
            "body": {"content": "{\"text\":\"hi\"}"},
            "mentions": [{"key": "@_user_1", "id": "ou_2", "name": "Alice"}],
        }))
        .unwrap();
        let m = item.normalize();
        assert_eq!(m.message_id, "om_1");
        assert_eq!(m.message_type, "text");
        assert_eq!(m.sender_id, "ou_1");
        assert_eq!(m.create_time, "1700000000000");
        assert_eq!(m.thread_id, "omt_1");
        assert_eq!(m.mentions.len(), 1);
        assert_eq!(m.mentions[0].key, "@_user_1");
    }

    #[test]
    fn content_disposition_filename_extraction() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"report.pdf\""),
            "report.pdf"
        );
        assert_eq!(
            filename_from_content_disposition("attachment; filename=a.png"),
            "a.png"
        );
        assert_eq!(filename_from_content_disposition(""), "");
        assert_eq!(filename_from_content_disposition("inline"), "");
    }

    #[test]
    fn form_encoding_matches_go_url_values() {
        assert_eq!(
            urlencode_pairs(&[
                ("sort_type".into(), "ByCreateTimeDesc".into()),
                ("page_size".into(), "10".into()),
            ]),
            "page_size=10&sort_type=ByCreateTimeDesc"
        );
        assert_eq!(form_encode("a b"), "a+b");
        assert_eq!(form_encode("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn binding_prompt_template_is_valid_card_json() {
        let raw = binding_prompt_template("https://cordy.example/lark/bind?token=x").unwrap();
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(doc["header"]["template"], serde_json::json!("blue"));
        assert_eq!(
            doc["elements"][1]["actions"][0]["url"],
            serde_json::json!("https://cordy.example/lark/bind?token=x")
        );
    }

    #[test]
    fn stub_client_refuses_every_call() {
        let stub = crate::client::StubApiClient::new();
        assert!(!stub.is_configured());
    }

    #[test]
    fn region_credentials_resolve_hosts() {
        let cfg = HttpClientConfig::default();
        let client = HttpApiClient::new(cfg);
        assert_eq!(
            client.resolve_base_url(&InstallationCredentials {
                region: Region::Lark,
                ..Default::default()
            }),
            "https://open.larksuite.com"
        );
        assert_eq!(
            client.resolve_base_url(&InstallationCredentials {
                region: Region::Feishu,
                ..Default::default()
            }),
            "https://open.feishu.cn"
        );
    }
}
