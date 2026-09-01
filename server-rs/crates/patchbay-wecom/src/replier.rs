//! Verdict-driven WeCom replies and private account-binding prompts.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::{ChatType, InboundMessage};
use patchbay_channel_engine::resolvers::{
    OutboundReplier as ReplierSeam, Outcome, ResolvedInstallation, Result as EngineResult,
};

use crate::outbound_relay::OutboundRelay;
use crate::senders_registry::SendersRegistry;

const AGENT_OFFLINE_TEXT: &str = "⚠️ 智能体当前不在线，你的消息已收到，等它上线后会处理。";
const AGENT_ARCHIVED_TEXT: &str = "⚠️ 该智能体已归档，无法回复。请联系工作区管理员。";
const FRESH_PENDING_TEXT: &str = "✅ 已准备开始新对话。你的下一条聊天消息将不带之前的上下文运行。";
const ISSUE_USAGE_TEXT: &str = "请填写任务标题，格式如下：\n\n`/issue <标题>`\n`[描述]`（可选）";
const BINDING_REUSED_TEXT: &str = "👋 绑定链接刚才已经发给你了，就在上方，请直接点击完成绑定。";
const BINDING_GROUP_ACK: &str = "👋 已把绑定链接私发给你，请在与我的单聊里点击完成绑定。";

#[derive(Debug, Clone)]
pub struct BindingToken {
    pub raw: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub reused: bool,
}

#[async_trait]
pub trait BindingMinter: Send + Sync {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        wecom_user_id: &str,
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
        wecom_user_id: &str,
    ) -> anyhow::Result<BindingToken> {
        let interval = sqlx::postgres::types::PgInterval {
            microseconds: 60 * 1_000_000,
            days: 0,
            months: 0,
        };
        if let Some(row) = patchbay_db::queries::channel::find_live_channel_binding_token(
            &self.pool,
            installation_id,
            crate::TYPE_WECOM,
            wecom_user_id,
            interval,
        )
        .await?
        {
            return Ok(BindingToken {
                raw: String::new(),
                expires_at: row.expires_at,
                reused: true,
            });
        }

        let mut bytes = [0_u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let token_hash = format!("{:x}", Sha256::digest(raw.as_bytes()));
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(15);
        patchbay_db::queries::channel::create_channel_binding_token(
            &self.pool,
            &token_hash,
            workspace_id,
            installation_id,
            crate::TYPE_WECOM,
            wecom_user_id,
            Some(expires_at),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("wecom: binding token was not persisted"))?;
        Ok(BindingToken {
            raw,
            expires_at,
            reused: false,
        })
    }
}

pub struct OutboundReplierConfig {
    pub binding: Option<Arc<dyn BindingMinter>>,
    pub senders: Arc<SendersRegistry>,
    pub relay: Option<Arc<OutboundRelay>>,
    pub app_url: String,
    pub binding_path: String,
}

pub struct OutboundReplier {
    binding: Option<Arc<dyn BindingMinter>>,
    senders: Arc<SendersRegistry>,
    relay: Option<Arc<OutboundRelay>>,
    app_url: String,
    binding_path: String,
}

impl OutboundReplier {
    pub fn new(cfg: OutboundReplierConfig) -> Self {
        let mut binding_path = if cfg.binding_path.is_empty() {
            "/wecom/bind".to_string()
        } else {
            cfg.binding_path
        };
        if !binding_path.starts_with('/') {
            binding_path.insert(0, '/');
        }
        Self {
            binding: cfg.binding,
            senders: cfg.senders,
            relay: cfg.relay,
            app_url: cfg.app_url.trim_end_matches('/').to_string(),
            binding_path,
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
            return self.post(&ctx, inst, msg, text).await;
        }
        match res.outcome.as_ref() {
            Some(outcome) if *outcome == Outcome::needs_binding() => {
                self.send_binding_prompt(ctx, inst, msg, res).await
            }
            Some(outcome) if *outcome == Outcome::agent_offline() => {
                self.post(&ctx, inst, msg, AGENT_OFFLINE_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::agent_archived() => {
                self.post(&ctx, inst, msg, AGENT_ARCHIVED_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::quota_exceeded() => {
                self.post(
                    &ctx,
                    inst,
                    msg,
                    patchbay_channel::quota_exceeded_notice_for_message(msg),
                )
                .await
            }
            Some(outcome) if *outcome == Outcome::quota_unavailable() => {
                self.post(
                    &ctx,
                    inst,
                    msg,
                    patchbay_channel::quota_unavailable_notice_for_message(msg),
                )
                .await
            }
            Some(outcome) if *outcome == Outcome::fresh_pending() => {
                self.post(&ctx, inst, msg, FRESH_PENDING_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::issue_usage() => {
                self.post(&ctx, inst, msg, ISSUE_USAGE_TEXT).await
            }
            Some(outcome) if *outcome == Outcome::ingested() && res.issue_id.is_some() => {
                self.post(&ctx, inst, msg, &issue_outcome_text(res)).await
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
        let sender = if res.sender.is_empty() {
            &msg.source.sender_id
        } else {
            &res.sender
        };
        if sender.is_empty() {
            anyhow::bail!("wecom: missing sender id");
        }
        let binding = self
            .binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("wecom: binding service not configured"))?;
        if self.app_url.is_empty() {
            anyhow::bail!("wecom: app url not configured");
        }
        let token = binding.mint(inst.workspace_id, inst.id, sender).await?;
        let text = if token.reused {
            BINDING_REUSED_TEXT.to_string()
        } else {
            let encoded: String =
                url::form_urlencoded::byte_serialize(token.raw.as_bytes()).collect();
            format!(
                "👋 请先绑定你的 Patchbay 账号，才能与我对话：\n{}{}?token={}\n（链接 15 分钟内有效）",
                self.app_url, self.binding_path, encoded
            )
        };
        self.post_private(&ctx, inst, sender, &text).await?;
        if msg.source.chat_type == ChatType::group() {
            self.post(&ctx, inst, msg, BINDING_GROUP_ACK).await?;
        }
        Ok(())
    }

    async fn post_private(
        &self,
        ctx: &CancellationToken,
        inst: &ResolvedInstallation,
        user_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        self.post_target(ctx, inst.id, user_id, 1, text).await
    }

    async fn post(
        &self,
        ctx: &CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        text: &str,
    ) -> anyhow::Result<()> {
        if msg.source.chat_id.is_empty() {
            anyhow::bail!("wecom: missing chat_id");
        }
        self.post_target(
            ctx,
            inst.id,
            &msg.source.chat_id,
            crate::ws_frame::aibot_chat_type_from_channel(&msg.source.chat_type),
            text,
        )
        .await
    }

    async fn post_target(
        &self,
        ctx: &CancellationToken,
        installation_id: Uuid,
        chat_id: &str,
        chat_type: i64,
        text: &str,
    ) -> anyhow::Result<()> {
        if let Some(sender) = self.senders.get(installation_id) {
            return sender.send_text_ctx(ctx, chat_id, chat_type, text).await;
        }
        let relay = self
            .relay
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("wecom: connection not ready"))?;
        relay
            .send_text(ctx, installation_id, chat_id, chat_type, text)
            .await
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
            tracing::warn!(installation_id = %inst.id, %error, "wecom verdict reply failed");
        }
    }
}

fn issue_outcome_text(res: &EngineResult) -> String {
    let id = if res.issue_identifier.is_empty() {
        format!("#{}", res.issue_number)
    } else {
        res.issue_identifier.clone()
    };
    let title = crate::markdown::break_member_links(res.issue_title.trim());
    match (res.issue_duplicate, title.is_empty()) {
        (true, true) => format!("⚠️ 未创建 —— 已存在进行中的 {id}"),
        (true, false) => format!("⚠️ 未创建 —— 已存在进行中的 {id} — {title}"),
        (false, true) => format!("✅ 已创建 {id}"),
        (false, false) => format!("✅ 已创建 {id} — {title}"),
    }
}
