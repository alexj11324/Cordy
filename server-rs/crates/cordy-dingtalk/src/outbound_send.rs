//! Port of `outbound_send.go`: the OUTBOUND send path shared by the
//! EventChatDone subscriber ([`crate::outbound`]) and the OutboundReplier
//! ([`crate::replier`]). It turns a reply body into one or more DingTalk
//! sampleMarkdown messages and posts them with the installation's
//! access_token, routing 1:1 vs group to the right robot endpoint.

use serde::Deserialize;

use crate::client::Client;
use crate::config::Credentials;
use crate::inbound::{CONV_TYPE_GROUP, CONV_TYPE_P2P};
use crate::markdown::{chunk_markdown, markdown_title};

/// Renders a {title, text} sampleMarkdown card.
const MSG_KEY_MARKDOWN: &str = "sampleMarkdown";

/// p2p (1:1) proactive send; group send.
pub(crate) const PATH_SEND_P2P: &str = "/v1.0/robot/oToMessages/batchSend";
pub(crate) const PATH_SEND_GROUP: &str = "/v1.0/robot/groupMessages/send";

/// The resolved DingTalk destination for a reply. ConversationType selects the
/// endpoint; staff_id is the recipient for a 1:1 send; conversation_id is the
/// group's openConversationId for a group send.
#[derive(Debug, Clone, Default)]
pub struct SendTarget {
    pub conversation_type: String,
    pub conversation_id: String,
    pub staff_id: String,
}

impl SendTarget {
    pub fn group(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_type: CONV_TYPE_GROUP.to_string(),
            conversation_id: conversation_id.into(),
            staff_id: String::new(),
        }
    }

    pub fn p2p(staff_id: impl Into<String>) -> Self {
        Self {
            conversation_type: CONV_TYPE_P2P.to_string(),
            conversation_id: String::new(),
            staff_id: staff_id.into(),
        }
    }
}

/// Posts replies for one installation. The robotCode + credentials come from
/// the installation; the shared Client owns the token cache and transport.
#[derive(Clone)]
pub(crate) struct Sender {
    client: std::sync::Arc<Client>,
    creds: Credentials,
}

#[derive(Debug, Default, Deserialize)]
struct SendResponse {
    #[serde(default, rename = "processQueryKey")]
    process_query_key: String,
}

impl Sender {
    pub fn new(client: std::sync::Arc<Client>, creds: Credentials) -> Self {
        Self { client, creds }
    }

    /// Delivers text to target as one or more sampleMarkdown messages (chunked
    /// under DingTalk's per-message byte cap). It returns the last message's
    /// send key. A 401 triggers one token refresh + retry, covering a
    /// server-side token revocation between cache fill and use.
    pub async fn send(&self, target: &SendTarget, text: &str) -> anyhow::Result<String> {
        if text.is_empty() {
            return Ok(String::new());
        }
        let title = markdown_title(text);
        let mut last_key = String::new();
        for chunk in chunk_markdown(text) {
            let param = serde_json::json!({"title": title, "text": chunk}).to_string();
            let key = self.send_one(target, &param).await?;
            last_key = key;
        }
        Ok(last_key)
    }

    /// Posts a single rendered message, refreshing the token once on 401.
    async fn send_one(&self, target: &SendTarget, msg_param: &str) -> anyhow::Result<String> {
        let (path, body) = self.request(target, msg_param)?;
        let mut retried = false;
        loop {
            let token = self
                .client
                .access_token(&self.creds.app_key, &self.creds.app_secret)
                .await
                .map_err(|e| anyhow::anyhow!("access token: {e:#}"))?;
            match self
                .client
                .post_json::<SendResponse>(path, &token, &body)
                .await
            {
                Ok(parsed) => {
                    return Ok(parsed.map(|r| r.process_query_key).unwrap_or_default());
                }
                Err(err) if !retried && crate::client::is_unauthorized(&err) => {
                    retried = true;
                    self.client.invalidate(&self.creds.app_key);
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Builds the endpoint + body for a target. A 1:1 send needs a recipient
    /// staff id; a group send needs the group's openConversationId.
    fn request(
        &self,
        target: &SendTarget,
        msg_param: &str,
    ) -> anyhow::Result<(&'static str, serde_json::Value)> {
        if target.conversation_type == CONV_TYPE_P2P {
            if target.staff_id.is_empty() {
                anyhow::bail!("dingtalk: 1:1 send missing recipient staff id");
            }
            return Ok((
                PATH_SEND_P2P,
                serde_json::json!({
                    "robotCode": self.creds.robot_code,
                    "userIds": [target.staff_id],
                    "msgKey": MSG_KEY_MARKDOWN,
                    "msgParam": msg_param,
                }),
            ));
        }
        if target.conversation_id.is_empty() {
            anyhow::bail!("dingtalk: group send missing conversation id");
        }
        Ok((
            PATH_SEND_GROUP,
            serde_json::json!({
                "robotCode": self.creds.robot_code,
                "openConversationId": target.conversation_id,
                "msgKey": MSG_KEY_MARKDOWN,
                "msgParam": msg_param,
            }),
        ))
    }
}
