//! Port of `replier.go`: the DingTalk OutboundReplier — the engine seam that
//! delivers a verdict-driven reply back to the user. It posts through the same
//! sender as the EventChatDone subscriber.
//!
//! Outcomes handled:
//!   - NeedsBinding: the sender is unbound. Mint a single-use binding token and
//!     reply with a "link your account" prompt pointing at the in-product
//!     redeem page. After they bind, their next message reaches the agent.
//!   - AgentOffline / AgentArchived: a status notice so the user is not left
//!     wondering why nothing happened.
//!   - FreshPending / IssueUsage: command confirmation or corrective guidance.
//!   - Ingested with a synchronously-created /issue: a confirmation carrying
//!     the issue identifier and title. Plain chat turns stay silent.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::InboundMessage;
use patchbay_channel_engine::parse_issue_command;
use patchbay_channel_engine::resolvers::{
    DropReason, OutboundReplier as ReplierSeam, Outcome, ResolvedInstallation,
    Result as EngineResult,
};

use crate::client::Client;
use crate::config::{decode_credentials, Decrypter};
use crate::db_row_from_platform;
use crate::outbound_send::{SendTarget, Sender};

pub const AGENT_OFFLINE_TEXT: &str =
    "⚠️ The agent is offline, so this message won't be processed automatically.";
pub const AGENT_ARCHIVED_TEXT: &str =
    "⚠️ This agent has been archived and can't respond. Please contact your workspace admin.";
pub const QUOTA_EXCEEDED_TEXT: &str =
    "⚠️ This workspace has reached its hosted messaging limit for the month. Existing runs will finish; upgrade to continue starting new runs.";
pub const FRESH_PENDING_TEXT: &str =
    "✅ Fresh start ready. Your next chat message will run without previous context.";
pub const ISSUE_USAGE_TEXT: &str =
    "Please include an issue title. Use:\n\n`/issue <title>`\n\n`[description]` (optional)";
pub const ISSUE_USAGE_WITH_MEDIA_TEXT: &str = "Please add a title and resend with the image (*image can come before or after the command*):\n\n`/issue <title>`\n\n`[description]` (optional)";
// Refusals for dropped /issue commands, carried over from the deleted
// pre-engine IssueCommandProcessor: without them the user's command vanishes
// with no signal that it will never be handled.
pub const ISSUE_NOT_MEMBER_TEXT: &str = "You're not a member of this Patchbay workspace, so I can't file an issue for you. Ask a workspace admin to invite you, then send the command again.";
pub const ISSUE_DISABLED_TEXT: &str = "This DingTalk robot isn't connected to Patchbay (or was disconnected). Ask a workspace admin to reconnect it.";

/// The binding-token surface the replier needs. [`crate::binding::BindingTokenService`]
/// satisfies it.
#[async_trait]
pub trait BindingMinter: Send + Sync {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        dingtalk_user_id: &str,
    ) -> anyhow::Result<crate::binding::BindingToken>;
}

/// Configures the replier. Binding + AppURL are required for the NeedsBinding
/// prompt to work; without them the prompt is skipped (the status and command
/// notices still fire).
pub struct OutboundReplierConfig {
    pub binding: Option<Arc<dyn BindingMinter>>,
    pub decrypt: Option<Arc<Decrypter>>,
    pub client: Option<Arc<Client>>,
    /// AppURL is the Patchbay web app host the user clicks into to redeem the
    /// binding token (e.g. <https://patchbay.example>). The bind page
    /// (/dingtalk/bind) is served by the web app, so the link must point at the
    /// app host, not the API host. Mirrors the Slack replier's AppURL.
    pub app_url: String,
    /// Default "/dingtalk/bind".
    pub binding_path: String,
}

/// Implements the engine OutboundReplier seam for DingTalk.
pub struct OutboundReplier {
    binding: Option<Arc<dyn BindingMinter>>,
    decrypt: Option<Arc<Decrypter>>,
    client: Arc<Client>,
    app_url: String,
    binding_path: String,
}

impl OutboundReplier {
    pub fn new(cfg: OutboundReplierConfig) -> Self {
        let client = cfg
            .client
            .unwrap_or_else(|| Arc::new(Client::new(None, "")));
        let mut binding_path = if cfg.binding_path.is_empty() {
            "/dingtalk/bind".to_string()
        } else {
            cfg.binding_path
        };
        if !binding_path.starts_with('/') {
            binding_path.insert(0, '/');
        }
        Self {
            binding: cfg.binding,
            decrypt: cfg.decrypt,
            client,
            app_url: cfg.app_url.trim_end_matches('/').to_string(),
            binding_path,
        }
    }

    async fn send_binding_prompt(
        &self,
        _ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) -> anyhow::Result<()> {
        let mut sender = res.sender.clone();
        if sender.is_empty() {
            sender.clone_from(&msg.source.sender_id);
        }
        if sender.is_empty() {
            anyhow::bail!("missing sender id");
        }
        let binding = self
            .binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("binding service not configured"))?;
        if self.app_url.is_empty() {
            anyhow::bail!("app url not configured");
        }
        let token = binding.mint(inst.workspace_id, inst.id, &sender).await?;
        let mut bind_url = url::Url::parse(&format!("{}{}", self.app_url, self.binding_path))
            .map_err(|e| anyhow::anyhow!("invalid app url: {e}"))?;
        bind_url.query_pairs_mut().append_pair("token", &token.raw);
        let text = format!(
            "👋 To start chatting with me, link your DingTalk account to Patchbay: [link your account]({bind_url})\n\n(This link expires in 15 minutes.)"
        );
        // Deliver the single-use binding link privately (1:1) to the sender,
        // never via targetFromMessage: in a group that would post the token
        // into the whole chat, where any other workspace member could redeem it
        // and bind the sender's DingTalk id to their own account (identity
        // misbinding).
        let target = SendTarget::p2p(sender);
        send_installation_text(&self.client, self.decrypt.as_deref(), inst, &target, &text)
            .await
            .map_err(|e| anyhow::anyhow!("post dingtalk binding prompt: {e:#}"))?;
        Ok(())
    }

    /// Resolves the installation's credentials from the carried platform row
    /// and sends text back into the originating conversation.
    async fn post(
        &self,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        text: &str,
    ) -> anyhow::Result<()> {
        send_installation_text(
            &self.client,
            self.decrypt.as_deref(),
            inst,
            &target_from_message(msg),
            text,
        )
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("post dingtalk reply: {e:#}"))
    }
}

#[async_trait]
impl ReplierSeam for OutboundReplier {
    /// Routes each outcome to its user-visible message. Errors are logged, not
    /// propagated: the replier runs detached from the inbound ACK path.
    async fn reply(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) {
        if let Some(text) = res.reply_text.as_deref() {
            if let Err(error) = self.post(inst, msg, text).await {
                tracing::warn!(installation_id = %inst.id, %error, "dingtalk hub reply failed");
            }
            return;
        }
        let Some(outcome) = res.outcome.clone() else {
            return;
        };
        let result = if outcome == Outcome::needs_binding() {
            self.send_binding_prompt(ctx, inst, msg, res)
                .await
                .map_err(|e| ("dingtalk replier: binding prompt failed", e))
        } else if outcome == Outcome::agent_offline() {
            self.post(inst, msg, AGENT_OFFLINE_TEXT)
                .await
                .map_err(|e| ("dingtalk replier: offline notice failed", e))
        } else if outcome == Outcome::agent_archived() {
            self.post(inst, msg, AGENT_ARCHIVED_TEXT)
                .await
                .map_err(|e| ("dingtalk replier: archived notice failed", e))
        } else if outcome == Outcome::quota_exceeded() {
            self.post(inst, msg, QUOTA_EXCEEDED_TEXT)
                .await
                .map_err(|e| ("dingtalk replier: quota notice failed", e))
        } else if outcome == Outcome::fresh_pending() {
            self.post(inst, msg, FRESH_PENDING_TEXT)
                .await
                .map_err(|e| ("dingtalk replier: fresh-start confirmation failed", e))
        } else if outcome == Outcome::issue_usage() {
            let text = if res.issue_usage_had_media {
                ISSUE_USAGE_WITH_MEDIA_TEXT
            } else {
                ISSUE_USAGE_TEXT
            };
            self.post(inst, msg, text)
                .await
                .map_err(|e| ("dingtalk replier: issue usage reply failed", e))
        } else if outcome == Outcome::ingested() {
            match res.issue_id {
                None => Ok(()),
                Some(_) => {
                    let text = if res.issue_duplicate {
                        issue_duplicate_text(res)
                    } else {
                        issue_created_text(res)
                    };
                    self.post(inst, msg, &text)
                        .await
                        .map_err(|e| ("dingtalk replier: issue outcome reply failed", e))
                }
            }
        } else if outcome == Outcome::dropped() {
            // Dropped /issue commands get a refusal so the sender is not left
            // waiting for an issue that will never be created; every other drop
            // (duplicates, unaddressed group chatter) stays silent.
            match dropped_reply_text(res, msg) {
                None => Ok(()),
                Some(text) => self
                    .post(inst, msg, &text)
                    .await
                    .map_err(|e| ("dingtalk replier: drop refusal failed", e)),
            }
        } else {
            Ok(())
        };
        if let Err((scope, err)) = result {
            tracing::warn!(
                installation_id = %inst.id,
                error = %err,
                "{scope}"
            );
        }
    }
}

/// Resolves an installation's credentials from the carried platform row and
/// sends text into target. Shared by the OutboundReplier and the ack notifier
/// so both proactive-send paths decode credentials identically.
pub(crate) async fn send_installation_text(
    client: &Arc<Client>,
    decrypt: Option<&Decrypter>,
    inst: &ResolvedInstallation,
    target: &SendTarget,
    text: &str,
) -> anyhow::Result<String> {
    let row = db_row_from_platform(inst)?;
    let creds = decode_credentials(&row.config, decrypt)
        .map_err(|e| anyhow::anyhow!("decode credentials: {e:#}"))?;
    Sender::new(client.clone(), creds).send(target, text).await
}

/// Builds the reply target from the inbound message's own routing identity
/// (used for the immediate binding/status replies, before any chat binding
/// exists).
pub(crate) fn target_from_message(msg: &InboundMessage) -> SendTarget {
    if msg.source.chat_type == patchbay_channel::ChatType::p2p() {
        SendTarget::p2p(msg.source.sender_id.clone())
    } else {
        SendTarget::group(msg.source.chat_id.clone())
    }
}

/// Reports whether msg is an /issue command explicitly addressed to the bot —
/// the gating the deleted pre-engine divert used. Only such messages warrant an
/// error/refusal reply: the sender asked for an action, so silence would read
/// as acceptance.
pub(crate) fn is_addressed_issue_command(msg: &InboundMessage) -> bool {
    if !msg.addressed_to_bot {
        return false;
    }
    let source = if msg.command_text.is_empty() {
        msg.text.as_str()
    } else {
        msg.command_text.as_str()
    };
    parse_issue_command(source).is_some()
}

/// Maps an OutcomeDropped result to a user-facing refusal.
fn dropped_reply_text(res: &EngineResult, msg: &InboundMessage) -> Option<String> {
    if !is_addressed_issue_command(msg) {
        return None;
    }
    let reason = res.drop_reason.clone()?;
    if reason == DropReason::non_workspace_member() {
        Some(ISSUE_NOT_MEMBER_TEXT.to_string())
    } else if reason == DropReason::revoked_installation() {
        Some(ISSUE_DISABLED_TEXT.to_string())
    } else {
        None
    }
}

fn issue_created_text(res: &EngineResult) -> String {
    let identifier = issue_result_identifier(res);
    if res.issue_title.is_empty() {
        format!("✅ Created {identifier}")
    } else {
        format!("✅ Created {identifier} — {}", res.issue_title)
    }
}

fn issue_duplicate_text(res: &EngineResult) -> String {
    let identifier = issue_result_identifier(res);
    if res.issue_title.is_empty() {
        format!("⚠️ Not created — active issue {identifier} already exists.")
    } else {
        format!(
            "⚠️ Not created — active issue {identifier} already exists: {}",
            res.issue_title
        )
    }
}

fn issue_result_identifier(res: &EngineResult) -> String {
    if !res.issue_identifier.is_empty() {
        return res.issue_identifier.clone();
    }
    if res.issue_number > 0 {
        return format!("#{}", res.issue_number);
    }
    res.issue_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| Uuid::nil().to_string())
}
