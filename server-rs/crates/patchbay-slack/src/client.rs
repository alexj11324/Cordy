//! Minimal Slack Web API client — the Rust stand-in for the slice of
//! `github.com/slack-go/slack` this adapter uses.
//!
//! Endpoints covered: conversations.history / conversations.replies (the
//! history reader), users.info (display-name resolution), reactions.add /
//! reactions.remove (the typing indicator), chat.postMessage (outbound Send),
//! apps.connections.open (Socket Mode URL), and response_url webhook POSTs
//! (ephemeral slash-command replies).
//!
//! Slack answers every Web API call with `{"ok": true, ...}` or
//! `{"ok": false, "error": "..."}`; both are folded into `Result` here.

use serde::Deserialize;

/// Default base URL of the Slack Web API. Tests point a client at a local
/// stub via [`SlackClient::with_api_url`].
pub const DEFAULT_API_URL: &str = "https://slack.com/api/";

/// One message as returned by conversations.history / conversations.replies.
/// Only the fields the history reader consumes are modeled; unknown fields
/// are ignored by serde.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub user: String,
    /// Slack timestamp "secs.micros" — the message id and paging cursor.
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub text: String,
    #[serde(rename = "reply_count", default)]
    pub reply_count: i64,
    #[serde(rename = "latest_reply", default)]
    pub latest_reply: String,
    #[serde(default)]
    pub username: String,
    #[serde(rename = "bot_id", default)]
    pub bot_id: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub blocks: Vec<Block>,
}

/// One legacy attachment (alerting/webhook bots carry their body here).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub pretext: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub fallback: String,
    #[serde(default)]
    pub fields: Vec<AttachmentField>,
    #[serde(default)]
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AttachmentField {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub value: String,
}

/// Block Kit block, internally tagged on `"type"`. Only the text-bearing
/// blocks are flattened; everything else falls into [`Block::Other`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Section {
        #[serde(default)]
        text: Option<TextObject>,
        #[serde(default)]
        fields: Vec<TextObject>,
    },
    Header {
        #[serde(default)]
        text: Option<TextObject>,
    },
    Markdown {
        #[serde(default)]
        text: String,
    },
    Context {
        #[serde(default)]
        elements: Vec<ContextElement>,
    },
    RichText {
        #[serde(default)]
        elements: Vec<RichTextElement>,
    },
    /// Interactive/media blocks and any future type — skipped when
    /// flattening, exactly like the Go switch's missing arms.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextObject {
    #[serde(default)]
    pub text: String,
}

/// Context-block elements: text objects plus images (skipped).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextElement {
    PlainText {
        #[serde(default)]
        text: String,
    },
    Mrkdwn {
        #[serde(default)]
        text: String,
    },
    Image {
        #[serde(default)]
        image_url: String,
        #[serde(default)]
        alt_text: String,
    },
    #[serde(other)]
    Other,
}

/// rich_text block element tree. Sections/quotes/preformatted hold inline
/// runs; lists recurse; anything else is skipped.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RichTextElement {
    RichTextSection {
        #[serde(default)]
        elements: Vec<RichTextSectionElement>,
    },
    RichTextQuote {
        #[serde(default)]
        elements: Vec<RichTextSectionElement>,
    },
    RichTextPreformatted {
        #[serde(default)]
        elements: Vec<RichTextSectionElement>,
    },
    RichTextList {
        #[serde(default)]
        elements: Vec<RichTextElement>,
    },
    #[serde(other)]
    Other,
}

/// Inline run inside a section/quote/preformatted. Only text and link runs
/// carry readable content; mentions/emoji/emoji decorations are skipped —
/// this is the plain body an agent needs, not a faithful re-render.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RichTextSectionElement {
    Text {
        #[serde(default)]
        text: String,
    },
    Link {
        #[serde(default)]
        text: String,
        url: String,
    },
    #[serde(other)]
    Other,
}

/// A Slack user, reduced to the name-resolution surface.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct User {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "real_name", default)]
    pub real_name: String,
    #[serde(default)]
    pub profile: UserProfile,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserProfile {
    #[serde(rename = "display_name", default)]
    pub display_name: String,
}

/// Paged conversation read result.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConversationHistoryResponse {
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(rename = "has_more", default)]
    pub has_more: bool,
}

/// The Socket Mode websocket URL minted by apps.connections.open.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectionsOpenResponse {
    pub url: String,
}

/// Minimal Slack Web API client over reqwest. One instance per bot token;
/// instances are cheap to clone and share a connection pool.
#[derive(Clone)]
pub struct SlackClient {
    http: reqwest::Client,
    api_url: String,
    token: String,
}

impl std::fmt::Debug for SlackClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the token.
        f.debug_struct("SlackClient")
            .field("api_url", &self.api_url)
            .field("token_set", &!self.token.is_empty())
            .finish()
    }
}

impl SlackClient {
    /// Builds a client authenticating with `token` against the real Slack API.
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_api_url(token, DEFAULT_API_URL)
    }

    /// Builds a client against an overridden API base (tests point it at a
    /// local stub server).
    pub fn with_api_url(token: impl Into<String>, api_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            // Callers bound latency with tokio timeouts where it matters
            // (media budget, typing clear); a flat cap keeps a wedged socket
            // from hanging a task forever.
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            http,
            api_url: api_url.into(),
            token: token.into(),
        }
    }

    /// GET-style API call with query parameters, folding Slack's ok/error
    /// envelope into Result.
    async fn get(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.api_url, method);
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .query(params)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("slack api {method}: {e}"))?;
        self.decode(method, resp).await
    }

    /// The API base URL this client posts against (Socket Mode dial reuses it
    /// for apps.connections.open with its own app-level token).
    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    async fn decode(
        &self,
        method: &str,
        resp: reqwest::Response,
    ) -> anyhow::Result<serde_json::Value> {
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("slack api {method}: http {status}");
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("slack api {method}: decode body: {e}"))?;
        match body.get("ok") {
            Some(serde_json::Value::Bool(true)) => Ok(body),
            _ => {
                let err = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown_error");
                anyhow::bail!("slack api {method}: {err}")
            }
        }
    }

    /// Reads a channel's recent top-level messages (newest-first from Slack;
    /// the caller re-orders). `latest` is the exclusive upper ts bound for
    /// back-paging (empty = most recent).
    pub async fn conversations_history(
        &self,
        channel_id: &str,
        latest: &str,
        limit: i64,
    ) -> anyhow::Result<ConversationHistoryResponse> {
        let mut params: Vec<(String, String)> = vec![
            ("channel".to_string(), channel_id.to_string()),
            ("limit".to_string(), limit.to_string()),
            ("inclusive".to_string(), "false".to_string()),
        ];
        if !latest.is_empty() {
            params.push(("latest".to_string(), latest.to_string()));
        }
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let body = self.get("conversations.history", &refs).await?;
        serde_json::from_value(body)
            .map_err(|e| anyhow::anyhow!("slack api conversations.history: decode: {e}"))
    }

    /// Reads one thread's replies (newest-first from Slack).
    pub async fn conversations_replies(
        &self,
        channel_id: &str,
        timestamp: &str,
        latest: &str,
        limit: i64,
    ) -> anyhow::Result<ConversationHistoryResponse> {
        let mut params: Vec<(String, String)> = vec![
            ("channel".to_string(), channel_id.to_string()),
            ("ts".to_string(), timestamp.to_string()),
            ("limit".to_string(), limit.to_string()),
            ("inclusive".to_string(), "false".to_string()),
        ];
        if !latest.is_empty() {
            params.push(("latest".to_string(), latest.to_string()));
        }
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let body = self.get("conversations.replies", &refs).await?;
        serde_json::from_value(body)
            .map_err(|e| anyhow::anyhow!("slack api conversations.replies: decode: {e}"))
    }

    /// Batch-resolves user info. Slack caps users.info at one id per call in
    /// practice for older workspaces but accepts comma-separated lists on
    /// current ones; we pass the joined list like slack-go does.
    pub async fn users_info(&self, user_ids: &[String]) -> anyhow::Result<Vec<User>> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = user_ids.join(",");
        let body = self.get("users.info", &[("user", ids.as_str())]).await?;
        #[derive(Deserialize)]
        struct UsersWrapper {
            #[serde(default)]
            user: Option<User>,
            #[serde(default)]
            users: Option<Vec<User>>,
        }
        let parsed: UsersWrapper = serde_json::from_value(body.clone())
            .map_err(|e| anyhow::anyhow!("slack api users.info: decode: {e}"))?;
        if let Some(users) = parsed.users {
            return Ok(users);
        }
        Ok(parsed.user.into_iter().collect())
    }

    /// Adds the named reaction to a message. Requires reactions:write.
    pub async fn reactions_add(
        &self,
        name: &str,
        channel_id: &str,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.post_form(
            "reactions.add",
            &[
                ("name", name),
                ("channel", channel_id),
                ("timestamp", timestamp),
            ],
        )
        .await
        .map(|_| ())
    }

    /// Removes the named reaction from a message.
    pub async fn reactions_remove(
        &self,
        name: &str,
        channel_id: &str,
        timestamp: &str,
    ) -> anyhow::Result<()> {
        self.post_form(
            "reactions.remove",
            &[
                ("name", name),
                ("channel", channel_id),
                ("timestamp", timestamp),
            ],
        )
        .await
        .map(|_| ())
    }

    /// Posts a message to a channel, optionally threaded or quote-replying.
    /// Returns the delivered message's ts.
    pub async fn chat_post_message(
        &self,
        channel_id: &str,
        text: &str,
        thread_ts: &str,
        reply_to: &str,
    ) -> anyhow::Result<String> {
        let mut params: Vec<(String, String)> = vec![
            ("channel".to_string(), channel_id.to_string()),
            ("text".to_string(), text.to_string()),
        ];
        if !thread_ts.is_empty() {
            params.push(("thread_ts".to_string(), thread_ts.to_string()));
        }
        if !reply_to.is_empty() {
            params.push(("reply_to".to_string(), reply_to.to_string()));
        }
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let body = self.post_form("chat.postMessage", &refs).await?;
        Ok(body
            .get("ts")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    async fn post_form(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.api_url, method);
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .form(params)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("slack api {method}: {e}"))?;
        self.decode(method, resp).await
    }

    /// Mints a fresh Socket Mode websocket URL for the app-level token this
    /// client carries (`xapp-`). The URL is single-use and expires quickly.
    pub async fn apps_connections_open(&self) -> anyhow::Result<String> {
        let url = format!("{}{}", self.api_url, "apps.connections.open");
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("slack api apps.connections.open: {e}"))?;
        let body = self.decode("apps.connections.open", resp).await?;
        let parsed: ConnectionsOpenResponse = serde_json::from_value(body)
            .map_err(|e| anyhow::anyhow!("slack api apps.connections.open: decode: {e}"))?;
        Ok(parsed.url)
    }

    /// POSTs an ephemeral reply to a slash command's signed response_url —
    /// no bot token required (Go slack.PostWebhookContext).
    pub async fn post_webhook_ephemeral(
        &self,
        response_url: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "response_type": "ephemeral",
            "text": text,
        });
        let resp = self
            .http
            .post(response_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("slack post webhook: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("slack post webhook: http {status}");
        }
        Ok(())
    }

    /// Downloads bytes from a Slack file URL with the bot token as bearer
    /// credential, following up to three redirects — each hop validated to be
    /// HTTPS on a Slack-owned host before it is taken, and the Authorization
    /// header restored on every hop (mirrors Go's CheckRedirect policy: a hop
    /// to a sibling such as files-origin.slack.com would otherwise arrive
    /// unauthenticated and answer with Slack's HTML login page).
    ///
    /// Returns the body (capped) and the response Content-Type without
    /// parameters.
    pub async fn download_file(
        &self,
        raw_url: &str,
        max_bytes: usize,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let mut current = reqwest::Url::parse(raw_url)
            .map_err(|_| anyhow::anyhow!("invalid file download URL"))?;
        validate_download_url(&current)?;
        let mut redirects = 0usize;
        loop {
            let resp = self
                .http
                .get(current.clone())
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("download file request failed: {e}"))?;
            if resp.status().is_redirection() {
                if redirects >= MAX_DOWNLOAD_REDIRECTS {
                    anyhow::bail!("too many redirects");
                }
                redirects += 1;
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow::anyhow!("redirect without location"))?;
                let next = resolve_redirect(&current, location)?;
                validate_download_url(&next)?;
                current = next;
                continue;
            }
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("download file: http {status}");
            }
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let data = resp
                .bytes()
                .await
                .map_err(|e| anyhow::anyhow!("read file: {e}"))?;
            if data.len() > max_bytes {
                anyhow::bail!("file exceeds the {} MiB limit", max_bytes >> 20);
            }
            return Ok((data.to_vec(), content_type));
        }
    }
}

const MAX_DOWNLOAD_REDIRECTS: usize = 3;

fn resolve_redirect(base: &reqwest::Url, location: &str) -> anyhow::Result<reqwest::Url> {
    base.join(location)
        .map_err(|_| anyhow::anyhow!("invalid redirect location"))
}

/// Validates a download destination before any request (or redirect hop) is
/// taken: HTTPS only, no userinfo, Slack-owned host only. This host check is
/// what keeps the bot token from ever leaving Slack's domains.
fn validate_download_url(parsed: &reqwest::Url) -> anyhow::Result<()> {
    if parsed.host_str().is_none_or(str::is_empty) || !parsed.username().is_empty() {
        anyhow::bail!("invalid file download URL shape");
    }
    if !parsed.scheme().eq_ignore_ascii_case("https") {
        anyhow::bail!("invalid file download URL scheme {:?}", parsed.scheme());
    }
    let Some(host) = parsed.host_str() else {
        anyhow::bail!("invalid file download URL shape");
    };
    if !crate::raw::is_slack_file_host(host) {
        anyhow::bail!("blocked non-Slack file download host");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_url_validation_rejects_non_slack_hosts() {
        assert!(validate_download_url(
            &reqwest::Url::parse("https://files.slack.com/x.png").unwrap()
        )
        .is_ok());
        assert!(
            validate_download_url(&reqwest::Url::parse("https://evil.com/x.png").unwrap()).is_err()
        );
        assert!(validate_download_url(
            &reqwest::Url::parse("http://files.slack.com/x.png").unwrap()
        )
        .is_err());
        assert!(validate_download_url(
            &reqwest::Url::parse("https://u:p@files.slack.com/x.png").unwrap()
        )
        .is_err());
    }

    #[test]
    fn redirect_resolution_joins_relative_locations() {
        let base = reqwest::Url::parse("https://files.slack.com/a").unwrap();
        assert_eq!(
            resolve_redirect(&base, "/b").unwrap().as_str(),
            "https://files.slack.com/b"
        );
        assert!(resolve_redirect(&base, "http://[").is_err());
    }

    #[tokio::test]
    async fn api_error_envelope_becomes_result_err() {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut s = stream;
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: 24\r\n\r\n{\"ok\":false,\"error\":\"x\"}")
                .await;
        });
        let client = SlackClient::with_api_url("xoxb-", format!("http://{addr}/"));
        let err = client
            .conversations_history("C1", "", 10)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("x"), "{err}");
    }
}
