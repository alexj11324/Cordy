//! Verdict replies, binding-token minting, and typing notifications.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::{ChatType, InboundMessage};
use patchbay_channel_engine::resolvers::{
    DropReason, OutboundReplier as ReplierSeam, Outcome, ResolvedInstallation,
    Result as EngineResult, TypingNotifier,
};

use crate::api::{BotApi, ReplyParameters, SendMessageParams};
use crate::config::{decode_credentials, DecrypterFn};

const AGENT_OFFLINE_TEXT: &str =
    "⚠️ 智能体当前离线，消息已记录。下次 daemon 上线后会自动继续处理。";
const AGENT_ARCHIVED_TEXT: &str = "⚠️ 该智能体已归档，无法回复。请联系工作区管理员。";
const BINDING_GROUP_HINT: &str = "请先私聊我发送一条消息，再完成 Patchbay 账号绑定。";
const FRESH_PENDING_TEXT: &str = "✅ 已准备开始新对话。你的下一条聊天消息将不带之前的上下文运行。";
const ISSUE_USAGE_TEXT: &str = "请填写任务标题，格式如下：\n\n/issue <标题>\n[描述]（可选）";
const ISSUE_NOT_MEMBER_TEXT: &str =
    "你不是该 Patchbay 工作区的成员，因此无法创建任务。请让工作区管理员邀请你后重试。";
const ISSUE_DISABLED_TEXT: &str =
    "该 Telegram 机器人未连接到 Patchbay（或已断开连接）。请让工作区管理员重新连接。";

#[derive(Debug, Clone)]
pub struct BindingToken {
    pub raw: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait BindingMinter: Send + Sync {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        telegram_user_id: &str,
    ) -> anyhow::Result<BindingToken>;
}

pub struct DbBindingMinter {
    pool: sqlx::PgPool,
}

impl DbBindingMinter {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BindingMinter for DbBindingMinter {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        telegram_user_id: &str,
    ) -> anyhow::Result<BindingToken> {
        let mut bytes = [0_u8; 32];
        let mut rng = rand::rngs::OsRng;
        rng.fill_bytes(&mut bytes);
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let token_hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(15);
        patchbay_db::queries::channel::create_channel_binding_token(
            &self.pool,
            &token_hash,
            workspace_id,
            installation_id,
            crate::TYPE_TELEGRAM,
            telegram_user_id,
            Some(expires_at),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("telegram: binding token was not persisted"))?;
        Ok(BindingToken { raw, expires_at })
    }
}

pub struct OutboundReplierConfig {
    pub binding: Option<Arc<dyn BindingMinter>>,
    pub decrypt: Option<Arc<DecrypterFn>>,
    pub app_url: String,
    pub binding_path: String,
    pub api_base: String,
}

pub struct OutboundReplier {
    binding: Option<Arc<dyn BindingMinter>>,
    decrypt: Option<Arc<DecrypterFn>>,
    app_url: String,
    binding_path: String,
    api_base: String,
}

impl OutboundReplier {
    pub fn new(cfg: OutboundReplierConfig) -> Self {
        let mut binding_path = if cfg.binding_path.is_empty() {
            "/telegram/bind".to_string()
        } else {
            cfg.binding_path
        };
        if !binding_path.starts_with('/') {
            binding_path.insert(0, '/');
        }
        Self {
            binding: cfg.binding,
            decrypt: cfg.decrypt,
            app_url: cfg.app_url.trim_end_matches('/').to_string(),
            binding_path,
            api_base: cfg.api_base,
        }
    }

    async fn reply_inner(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) -> anyhow::Result<()> {
        if let Some(text) = res.reply_text.as_deref() {
            return self.post(ctx, inst, msg, text).await;
        }
        match res.outcome.as_ref() {
            Some(outcome) if *outcome == Outcome::needs_binding() => {
                self.send_binding_prompt(ctx, inst, msg, res).await
            }
            Some(outcome) if *outcome == Outcome::agent_offline() => {
                self.post(ctx, inst, msg, AGENT_OFFLINE_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::agent_archived() => {
                self.post(ctx, inst, msg, AGENT_ARCHIVED_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::quota_exceeded() => {
                self.post(
                    ctx,
                    inst,
                    msg,
                    patchbay_channel::quota_exceeded_notice_for_message(msg),
                )
                .await
            }
            Some(outcome) if *outcome == Outcome::fresh_pending() => {
                self.post(ctx, inst, msg, FRESH_PENDING_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::issue_usage() => {
                self.post(ctx, inst, msg, ISSUE_USAGE_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::ingested() && res.issue_id.is_some() => {
                self.post(ctx, inst, msg, &issue_outcome_text(res)).await
            }
            Some(outcome) if *outcome == Outcome::dropped() => {
                let text = dropped_reply_text(res, msg);
                if text.is_empty() {
                    Ok(())
                } else {
                    self.post(ctx, inst, msg, text).await
                }
            }
            _ => Ok(()),
        }
    }

    async fn send_binding_prompt(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) -> anyhow::Result<()> {
        if msg.source.chat_type == ChatType::group() {
            return self.post(ctx, inst, msg, BINDING_GROUP_HINT).await;
        }
        let sender = if res.sender.is_empty() {
            &msg.source.sender_id
        } else {
            &res.sender
        };
        if sender.is_empty() {
            anyhow::bail!("telegram replier: missing sender id");
        }
        let binding = self
            .binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("telegram replier: binding service not configured"))?;
        if self.app_url.is_empty() {
            anyhow::bail!("telegram replier: app url not configured");
        }
        let token = binding.mint(inst.workspace_id, inst.id, sender).await?;
        let encoded: String = url::form_urlencoded::byte_serialize(token.raw.as_bytes()).collect();
        let text = format!(
            "👋 要开始和我对话，请先绑定你的 Patchbay 账号：\n{}{}?token={}\n（链接 15 分钟内有效）",
            self.app_url, self.binding_path, encoded
        );
        self.post(ctx, inst, msg, &text).await
    }

    async fn post(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        text: &str,
    ) -> anyhow::Result<()> {
        let api = installation_api(inst, self.decrypt.as_deref(), &self.api_base)?;
        let chat_id = msg
            .source
            .chat_id
            .parse::<i64>()
            .map_err(|error| anyhow::anyhow!("telegram replier: bad chat id: {error}"))?;
        let thread_id = msg.source.thread_id.parse().unwrap_or(0);
        let reply_to = crate::sender::parse_message_ref(&msg.message_id);
        let params = SendMessageParams {
            chat_id,
            text: text.to_string(),
            message_thread_id: thread_id,
            reply_parameters: (reply_to != 0).then_some(ReplyParameters {
                message_id: reply_to,
                allow_sending_without_reply: true,
            }),
            ..Default::default()
        };
        tokio::select! {
            _ = ctx.cancelled() => Ok(()),
            result = api.send_message(&params) => result.map(|_| ()),
        }
    }
}

#[async_trait]
impl ReplierSeam for OutboundReplier {
    async fn reply(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        res: &EngineResult,
    ) {
        if let Err(error) = self.reply_inner(ctx, inst, msg, res).await {
            tracing::warn!(installation_id = %inst.id, %error, "telegram verdict reply failed");
        }
    }
}

pub struct TypingIndicator {
    decrypt: Option<Arc<DecrypterFn>>,
    api_base: String,
}

impl TypingIndicator {
    pub fn new(decrypt: Option<Arc<DecrypterFn>>, api_base: String) -> Self {
        Self { decrypt, api_base }
    }
}

#[async_trait]
impl TypingNotifier for TypingIndicator {
    async fn on_ingested(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        _session_id: Uuid,
    ) {
        let result = async {
            let api = installation_api(inst, self.decrypt.as_deref(), &self.api_base)?;
            let chat_id = msg.source.chat_id.parse::<i64>()?;
            let thread_id = msg.source.thread_id.parse().unwrap_or(0);
            tokio::select! {
                _ = ctx.cancelled() => Ok(()),
                result = api.send_chat_action(chat_id, thread_id) => result,
            }
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(installation_id = %inst.id, %error, "telegram typing notification failed");
        }
    }

    async fn on_settled(&self, _ctx: CancellationToken, _session_id: Uuid) {}
}

pub(crate) fn installation_api(
    inst: &ResolvedInstallation,
    decrypt: Option<&DecrypterFn>,
    api_base: &str,
) -> anyhow::Result<BotApi> {
    let row = inst
        .platform
        .clone()
        .downcast::<patchbay_db::models::ChannelInstallation>()
        .map_err(|_| anyhow::anyhow!("telegram: installation platform row unavailable"))?;
    let raw = serde_json::to_vec(&row.config)?;
    let credentials = decode_credentials(&raw, decrypt)?;
    if credentials.bot_token.is_empty() {
        anyhow::bail!("telegram: installation has no bot token");
    }
    Ok(BotApi::new(api_base, &credentials.bot_token))
}

fn issue_outcome_text(res: &EngineResult) -> String {
    let id = if !res.issue_identifier.is_empty() {
        res.issue_identifier.clone()
    } else if res.issue_number > 0 {
        format!("#{}", res.issue_number)
    } else {
        res.issue_id.map(|id| id.to_string()).unwrap_or_default()
    };
    let title = res.issue_title.trim();
    match (res.issue_duplicate, title.is_empty()) {
        (true, true) => format!("⚠️ 未创建：已存在进行中的任务 {id}。"),
        (true, false) => format!("⚠️ 未创建：已存在进行中的任务 {id} — {title}"),
        (false, true) => format!("✅ 已创建 {id}"),
        (false, false) => format!("✅ 已创建 {id} — {title}"),
    }
}

fn dropped_reply_text(res: &EngineResult, msg: &InboundMessage) -> &'static str {
    if !is_addressed_issue_command(msg) {
        return "";
    }
    match res.drop_reason.as_ref() {
        Some(reason) if *reason == DropReason::non_workspace_member() => ISSUE_NOT_MEMBER_TEXT,
        Some(reason) if *reason == DropReason::revoked_installation() => ISSUE_DISABLED_TEXT,
        _ => "",
    }
}

fn is_addressed_issue_command(msg: &InboundMessage) -> bool {
    if !msg.addressed_to_bot {
        return false;
    }
    let source = if msg.command_text.is_empty() {
        &msg.text
    } else {
        &msg.command_text
    };
    patchbay_channel_engine::parse_issue_command(source).is_some()
}
