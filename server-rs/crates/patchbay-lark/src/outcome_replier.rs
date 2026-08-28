//! The verdict-driven outbound replier.
//!
//! Reacts to the Dispatcher's verdict by posting the appropriate Lark-side
//! reply card. NeedsBinding sends the binding prompt to the sender's open_id,
//! AgentOffline / AgentArchived send a status notice into the chat, and
//! FreshPending / IssueUsage send command guidance. OutcomeIngested is owned by
//! the Patcher (task lifecycle); OutcomeDropped is silent.
//!
//! Reply is best-effort by design: a transient Lark outage MUST NOT fail the
//! inbound pipeline. Errors are logged and swallowed.

use async_trait::async_trait;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::binding_token::BindingTokenMinter;
use crate::client::{ApiClient, BindingPromptParams, SendCardParams, SendTextParams};
use crate::feishu_types::{DispatchResult, InboundMessage};
use crate::installation::CredentialsResolver;
use crate::outbound::{inbound_reply_target, send_with_thread_fallback};
use crate::resolvers::OutcomeReplier;
use crate::store::Installation;
use crate::types::OpenId;

/// The narrow DB surface the replier needs: the agent name shown on cards.
#[async_trait]
pub trait AgentNameLookup: Send + Sync {
    async fn get_agent_name(&self, id: Uuid) -> anyhow::Result<String>;
}

#[async_trait]
impl AgentNameLookup for crate::channel_store::ChannelStore {
    async fn get_agent_name(&self, id: Uuid) -> anyhow::Result<String> {
        Ok(patchbay_db::queries::agent::get_agent(self.pool(), id)
            .await?
            .map(|agent| agent.name)
            .unwrap_or_default())
    }
}

/// The safe default when Lark is wired without an outbound APIClient (stub) or
/// without a BindingTokenService. It logs each outcome that would have
/// produced a reply so an operator can see the gap in production logs.
pub struct NoopOutcomeReplier;

#[async_trait]
impl OutcomeReplier for NoopOutcomeReplier {
    async fn reply(
        &self,
        _ctx: CancellationToken,
        inst: &Installation,
        msg: &InboundMessage,
        res: &DispatchResult,
    ) {
        let outcome = res.outcome_str();
        if matches!(
            outcome,
            "needs_binding" | "agent_offline" | "agent_archived" | "fresh_pending" | "issue_usage"
        ) {
            tracing::warn!(
                outcome = %outcome,
                installation_id = %inst.id,
                chat_id = %msg.chat_id.0,
                open_id = %msg.sender_open_id.0,
                "lark outcome replier: outbound reply skipped (replier not wired)"
            );
        }
    }
}

/// Returns the no-op replier. Used as the fallback when the production wiring
/// is incomplete (e.g. stub APIClient, no binding token service).
pub fn new_noop_outcome_replier() -> Arc<dyn OutcomeReplier> {
    Arc::new(NoopOutcomeReplier)
}

/// The production OutcomeReplier. It composes:
///
///   - APIClient — to send the binding prompt card (open_id-targeted) and the
///     offline/archived notice cards (chat_id-targeted).
///   - BindingTokenService — to mint a one-shot binding token for the
///     NeedsBinding flow.
///   - CredentialsResolver — to decrypt app_secret per call (the plaintext
///     secret never lives on the in-memory installation row).
///   - AgentNameLookup — for the agent name shown on cards.
///
/// The replier is constructed once at boot and shared across the Hub's
/// supervisor goroutines; all dependencies must be goroutine-safe.
pub struct LarkOutcomeReplier {
    client: Arc<dyn ApiClient>,
    binding_svc: Arc<dyn BindingTokenMinter>,
    credentials: Arc<dyn CredentialsResolver>,
    queries: Arc<dyn AgentNameLookup>,
    /// e.g. https://patchbay.example, trailing slash trimmed
    app_url: String,
    /// path component of the binding URL, default "/lark/bind"
    binding_path: String,
}

/// Wires the production replier. `app_url` is the Patchbay web app host the user
/// clicks into to redeem the binding token or open an issue. It comes from
/// PATCHBAY_APP_URL and is intentionally separate from PATCHBAY_PUBLIC_URL, which is
/// the backend/API public URL used for webhook and daemon-facing endpoints.
/// Empty means the binding flow can only log the open_id, not produce a
/// clickable card.
pub struct OutcomeReplierConfig {
    pub api_client: Option<Arc<dyn ApiClient>>,
    pub binding_svc: Option<Arc<dyn BindingTokenMinter>>,
    pub credentials: Option<Arc<dyn CredentialsResolver>>,
    pub queries: Option<Arc<dyn AgentNameLookup>>,
    pub app_url: String,
    pub binding_path: String,
}

/// Validates the configuration and returns the production replier; missing
/// dependencies fall back to noop so the boot path stays robust on
/// partially-configured deployments.
pub fn new_lark_outcome_replier(cfg: OutcomeReplierConfig) -> Arc<dyn OutcomeReplier> {
    let Some(client) = cfg.api_client else {
        return new_noop_outcome_replier();
    };
    let Some(binding_svc) = cfg.binding_svc else {
        return new_noop_outcome_replier();
    };
    let Some(credentials) = cfg.credentials else {
        return new_noop_outcome_replier();
    };
    let Some(queries) = cfg.queries else {
        return new_noop_outcome_replier();
    };
    if !client.is_configured() {
        tracing::warn!(
            "lark outcome replier: APIClient.IsConfigured()=false; downgrading to noop replier"
        );
        return new_noop_outcome_replier();
    }
    if cfg.app_url.is_empty() {
        tracing::warn!(
            "lark outcome replier: PATCHBAY_APP_URL not set; binding prompt CTA will not work"
        );
    }
    let mut binding_path = cfg.binding_path;
    if binding_path.is_empty() {
        binding_path = "/lark/bind".to_string();
    }
    if !binding_path.starts_with('/') {
        binding_path = format!("/{binding_path}");
    }
    Arc::new(LarkOutcomeReplier {
        client,
        binding_svc,
        credentials,
        queries,
        app_url: cfg.app_url.trim_end_matches('/').to_string(),
        binding_path,
    })
}

/// Reads carefully — the match is the SOURCE OF TRUTH for which outcomes
/// generate a reply, and a missing branch silently drops the user-visible side
/// effect.
#[async_trait]
impl OutcomeReplier for LarkOutcomeReplier {
    async fn reply(
        &self,
        _ctx: CancellationToken,
        inst: &Installation,
        msg: &InboundMessage,
        res: &DispatchResult,
    ) {
        let result = match res.outcome_str() {
            "needs_binding" => self.send_binding_prompt(_ctx.clone(), inst, res).await,
            "agent_offline" => {
                self.send_chat_notice(_ctx.clone(), inst, msg, AGENT_OFFLINE_COPY)
                    .await
            }
            "agent_archived" => {
                self.send_chat_notice(_ctx.clone(), inst, msg, AGENT_ARCHIVED_COPY)
                    .await
            }
            "fresh_pending" => {
                self.send_chat_notice(_ctx.clone(), inst, msg, FRESH_PENDING_COPY)
                    .await
            }
            "issue_usage" => {
                let copy = if res.issue_usage_had_media {
                    ISSUE_USAGE_WITH_MEDIA_COPY
                } else {
                    ISSUE_USAGE_COPY
                };
                self.send_chat_notice(_ctx.clone(), inst, msg, copy).await
            }
            // The agent's chat reply itself goes through the Patcher. An /issue
            // command gets an immediate product result: either the newly
            // created issue or the active duplicate that blocked it. Gate on
            // IssueID presence so a plain chat message stays silent here.
            "ingested" => match res.issue_id {
                Some(_) => self.send_issue_outcome(_ctx.clone(), inst, msg, res).await,
                None => Ok(()),
            },
            // Dropped is informational; no user-visible reply.
            _ => Ok(()),
        };
        if let Err(err) = result {
            tracing::warn!(
                installation_id = %inst.id,
                chat_id = %msg.chat_id.0,
                outcome = %res.outcome_str(),
                error = %err,
                "lark outcome replier: reply failed"
            );
        }
    }
}

impl LarkOutcomeReplier {
    async fn send_binding_prompt(
        &self,
        _ctx: CancellationToken,
        inst: &Installation,
        res: &DispatchResult,
    ) -> anyhow::Result<()> {
        if res.sender_open_id.0.is_empty() {
            anyhow::bail!("missing sender open_id");
        }
        if self.app_url.is_empty() {
            anyhow::bail!("app_url not configured");
        }
        let token = self
            .binding_svc
            .mint(
                inst.workspace_id,
                inst.id,
                &OpenId(res.sender_open_id.0.clone()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("mint binding token: {e:#}"))?;
        let bind_url = binding_url(&self.app_url, &self.binding_path, &token.raw);
        let creds = self.installation_credentials(inst).await?;
        self.client
            .send_binding_prompt_card(BindingPromptParams {
                installation_id: creds,
                open_id: OpenId(res.sender_open_id.0.clone()),
                bind_url,
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e:#}"))
    }

    /// Posts either the created confirmation or active-duplicate conflict as
    /// plain text, with a link to the relevant issue when configured.
    async fn send_issue_outcome(
        &self,
        _ctx: CancellationToken,
        inst: &Installation,
        msg: &InboundMessage,
        res: &DispatchResult,
    ) -> anyhow::Result<()> {
        if msg.chat_id.0.is_empty() {
            anyhow::bail!("missing chat_id");
        }
        let creds = self.installation_credentials(inst).await?;
        let text = if res.issue_duplicate {
            issue_duplicate_text(res, &self.app_url)
        } else {
            issue_created_text(res, &self.app_url)
        };
        // Share the Patcher's classified fallback: a thread reply that fails
        // because the topic cannot receive it (recalled trigger, topics
        // disabled, aggregated message) falls back to a chat-level send so the
        // product result is not lost; transport/5xx/rate-limit failures stay
        // failures rather than leaking into the group chat.
        send_with_thread_fallback("send issue outcome text", inbound_reply_target(msg), |t| {
            let client = Arc::clone(&self.client);
            let creds = creds.clone();
            let chat_id = msg.chat_id.clone();
            let text = text.clone();
            async move {
                client
                    .send_text_message(SendTextParams {
                        installation_id: creds,
                        chat_id,
                        text,
                        reply_target: t,
                    })
                    .await
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
            }
        })
        .await
    }

    async fn send_chat_notice(
        &self,
        _ctx: CancellationToken,
        inst: &Installation,
        msg: &InboundMessage,
        body: &str,
    ) -> anyhow::Result<()> {
        if msg.chat_id.0.is_empty() {
            anyhow::bail!("missing chat_id");
        }
        let creds = self.installation_credentials(inst).await?;
        let header = match self.queries.get_agent_name(inst.agent_id).await {
            Ok(name) if !name.is_empty() => name,
            _ => "Patchbay".to_string(),
        };
        let card_json = render_notice_card(&header, body)
            .map_err(|e| anyhow::anyhow!("render notice card: {e}"))?;
        // Same classified fallback as sendIssueOutcome: only thread-reply
        // failures that mean the topic cannot receive the message fall back to
        // a chat-level send; ambiguous/transport failures stay failures.
        send_with_thread_fallback("send notice card", inbound_reply_target(msg), move |t| {
            let client = Arc::clone(&self.client);
            let creds = creds.clone();
            let chat_id = msg.chat_id.clone();
            let card_json = card_json.clone();
            async move {
                client
                    .send_interactive_card(SendCardParams {
                        installation_id: creds,
                        chat_id,
                        card_json,
                        reply_target: t,
                    })
                    .await
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
            }
        })
        .await
    }

    async fn installation_credentials(
        &self,
        inst: &Installation,
    ) -> anyhow::Result<crate::client::InstallationCredentials> {
        crate::installation::installation_credentials_for(self.credentials.as_ref(), inst)
            .map_err(|e| anyhow::anyhow!("decrypt app_secret: {e:#}"))
    }
}

fn binding_url(app_url: &str, binding_path: &str, raw_token: &str) -> String {
    use url::form_urlencoded::Serializer;
    let mut query = Serializer::new(String::new());
    query.append_pair("token", raw_token);
    format!("{app_url}{binding_path}?{}", query.finish())
}

/// Composes the user-facing confirmation. Identifier always wins over a bare
/// number — DispatchResult.IssueIdentifier already encodes the workspace
/// prefix when available. AppURL is optional: when empty (self-host operators
/// who haven't configured PATCHBAY_APP_URL) the message still confirms the issue,
/// just without a deep link the user can tap.
pub fn issue_created_text(res: &DispatchResult, app_url: &str) -> String {
    let identifier = identifier_or_number(res);
    let title = res.issue_title.trim();
    let line = if title.is_empty() {
        format!("Created {identifier}")
    } else {
        format!("Created {identifier} — {title}")
    };
    finish_issue_line(line, app_url)
}

pub fn issue_duplicate_text(res: &DispatchResult, app_url: &str) -> String {
    let identifier = identifier_or_number(res);
    let title = res.issue_title.trim();
    let line = if title.is_empty() {
        format!("Not created — active issue {identifier} already exists.")
    } else {
        format!("Not created — active issue {identifier} already exists: {title}")
    };
    finish_issue_line(line, app_url)
}

fn identifier_or_number(res: &DispatchResult) -> String {
    if res.issue_identifier.is_empty() {
        format!("#{}", res.issue_number)
    } else {
        res.issue_identifier.clone()
    }
}

fn finish_issue_line(line: String, app_url: &str) -> String {
    if app_url.is_empty() {
        return line;
    }
    format!("{line}\n{}/issues/{}", app_url.trim_end_matches('/'), {
        // The identifier is the last whitespace-delimited token before the
        // title separator (or the whole line when no title) — recover it the
        // same way Go re-uses its `identifier` local in both composers.
        identifier_of(&line)
    })
}

fn identifier_of(line: &str) -> String {
    let head = line.split(" — ").next().unwrap_or(line);
    match head.rsplit(' ').find(|t| !t.is_empty()) {
        Some(tok) => tok.to_string(),
        None => head.to_string(),
    }
}

/// Produces a minimal text-only interactive card for the offline / archived
/// dispatch outcomes. Lark requires update_multi=true on every card we may
/// patch later; these notice cards are one-shot, so update_multi is left false
/// (the card stays as-is).
pub fn render_notice_card(header: &str, body: &str) -> Result<String, serde_json::Error> {
    let doc = serde_json::json!({
        "config": {"wide_screen_mode": true},
        "header": {
            "template": "grey",
            "title": {"tag": "plain_text", "content": header},
        },
        "elements": [
            {
                "tag": "div",
                "text": {"tag": "plain_text", "content": body},
            }
        ],
    });
    serde_json::to_string(&doc)
}

/// The user-visible Chinese strings for the two daemon/agent unavailability
/// outcomes. They match the §4.6 design: an offline agent will run when the
/// daemon comes back; an archived agent needs operator action.
pub const AGENT_OFFLINE_COPY: &str =
    "Agent 当前离线，消息已记录。下次 daemon 上线后会自动继续处理。";
pub const AGENT_ARCHIVED_COPY: &str =
    "这个 Agent 已被归档，无法继续处理消息。请联系工作区管理员恢复或重新绑定。";
pub const FRESH_PENDING_COPY: &str =
    "✅ 已准备开始新对话。你的下一条聊天消息将不带之前的上下文运行。";
pub const ISSUE_USAGE_COPY: &str =
    "请填写任务标题，格式如下：\n\n`/issue <标题>`\n`[描述]`（可选）";
pub const ISSUE_USAGE_WITH_MEDIA_COPY: &str =
    "请添加标题，并与图片或视频一起重新发送（*图片或视频可以位于命令之前或之后*）：\n\n`/issue <标题>`\n`[描述]`（可选）";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_texts_compose_confirmation_and_conflict() {
        let mut res = DispatchResult {
            issue_number: 41,
            ..Default::default()
        };

        // Bare number, no app URL.
        assert_eq!(issue_created_text(&res, ""), "Created #41");

        // Identifier wins over number; link appended.
        res.issue_identifier = "PATCHBAY-41".to_string();
        assert_eq!(
            issue_created_text(&res, "https://patchbay.example"),
            "Created PATCHBAY-41\nhttps://patchbay.example/issues/PATCHBAY-41"
        );

        // Title included when present; trailing slash trimmed on the URL.
        res.issue_title = "  Fix login  ".to_string();
        assert_eq!(
            issue_created_text(&res, "https://patchbay.example/"),
            "Created PATCHBAY-41 — Fix login\nhttps://patchbay.example/issues/PATCHBAY-41"
        );

        // Duplicate wording.
        let dup = DispatchResult {
            issue_identifier: "PATCHBAY-7".to_string(),
            issue_title: "Existing".to_string(),
            ..Default::default()
        };
        assert_eq!(
            issue_duplicate_text(&dup, ""),
            "Not created — active issue PATCHBAY-7 already exists: Existing"
        );
        let dup_bare = DispatchResult {
            issue_number: 9,
            ..Default::default()
        };
        assert_eq!(
            issue_duplicate_text(&dup_bare, ""),
            "Not created — active issue #9 already exists."
        );
    }

    #[test]
    fn binding_url_encodes_the_raw_token_once() {
        assert_eq!(
            binding_url("https://patchbay.example", "/lark/bind", "a+b=c"),
            "https://patchbay.example/lark/bind?token=a%2Bb%3Dc"
        );
    }

    #[test]
    fn notice_card_is_valid_lark_card_json() {
        let raw = render_notice_card("Patchbay", "hello").unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["header"]["template"], "grey");
        assert_eq!(v["header"]["title"]["content"], "Patchbay");
        assert_eq!(v["elements"][0]["text"]["content"], "hello");
        // One-shot cards stay patchable-off.
        assert!(v
            .get("config")
            .and_then(|c| c.get("update_multi"))
            .is_none());
    }

    #[test]
    fn copies_are_preserved_verbatim() {
        assert_eq!(
            AGENT_OFFLINE_COPY,
            "Agent 当前离线，消息已记录。下次 daemon 上线后会自动继续处理。"
        );
        assert_eq!(
            AGENT_ARCHIVED_COPY,
            "这个 Agent 已被归档，无法继续处理消息。请联系工作区管理员恢复或重新绑定。"
        );
        assert_eq!(
            FRESH_PENDING_COPY,
            "✅ 已准备开始新对话。你的下一条聊天消息将不带之前的上下文运行。"
        );
    }
}
