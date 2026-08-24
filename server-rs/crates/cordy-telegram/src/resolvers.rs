//! The Telegram ResolverSet: DB-backed installation/identity/dedup/
//! session seams the shared Router consumes.
//!
//! Port of `server/internal/integrations/telegram/resolvers.go`.

use std::sync::Arc;

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use cordy_channel::InboundMessage;
use cordy_channel_engine::resolvers::{
    AppendParams as EngineAppendParams, AppendResult, BindMediaParams as EngineBindMediaParams,
    Deduper, DropReason, EnsureSessionParams, IdentityResolver, InstallationResolver,
    ResolvedIdentity, ResolvedInstallation, ResolverError, ResolverSet, SessionBinder,
    TypingNotifier,
};
use cordy_channel_engine::session::{
    AppendInput, BindMediaInput, ChatSession, EnsureSessionInput, SessionTitles,
};
use cordy_db::queries::channel::{
    claim_channel_inbound_dedup, get_channel_installation_by_app_id,
    get_channel_user_binding_by_user_id, mark_channel_inbound_dedup_processed,
    record_channel_inbound_drop, release_channel_inbound_dedup,
};
use cordy_db::queries::member::get_member_by_user_and_workspace;

/// Origin type stamped on issues created from a Telegram chat.
pub const ORIGIN_TELEGRAM_CHAT: &str = "telegram_chat";

#[derive(Debug, Clone, serde::Serialize)]
pub struct TelegramBindingConfig {
    pub chat_id: String,
}

/// Session-isolation key + reply-thread routing for Telegram. A group
/// topic (thread) splits one chat into distinct sessions.
pub fn telegram_session_routing(msg: &InboundMessage) -> (String, serde_json::Value, String) {
    let chat_id = msg.source.chat_id.clone();
    let cfg = serde_json::to_value(TelegramBindingConfig {
        chat_id: chat_id.clone(),
    })
    .unwrap_or_else(|_| json!({}));
    if msg.source.chat_type == cordy_channel::ChatType::group() && !msg.source.thread_id.is_empty()
    {
        (
            format!("{chat_id}:{}", msg.source.thread_id),
            cfg,
            msg.source.thread_id.clone(),
        )
    } else {
        (chat_id, cfg, msg.source.thread_id.clone())
    }
}

fn decode_telegram_raw(msg: &InboundMessage) -> anyhow::Result<crate::TelegramRawEvent> {
    if msg.raw.is_null() {
        anyhow::bail!("telegram: inbound message Raw is empty");
    }
    serde_json::from_value(msg.raw.clone())
        .map_err(|e| anyhow::anyhow!("decode telegram inbound raw: {e}"))
}

struct InstallationResolverImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl InstallationResolver for InstallationResolverImpl {
    async fn resolve_installation(
        &self,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        let raw = decode_telegram_raw(msg)?;
        let Some(inst) =
            get_channel_installation_by_app_id(&self.pool, crate::TYPE_TELEGRAM, &raw.bot_id)
                .await?
        else {
            return Err(ResolverError::InstallationNotFound.into());
        };
        Ok(ResolvedInstallation {
            id: inst.id,
            workspace_id: inst.workspace_id,
            agent_id: inst.agent_id,
            route_revision: 0,
            installer_user_id: inst.installer_user_id,
            active: inst.status == "active",
            platform: Arc::new(inst),
        })
    }
}

struct IdentityResolverImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl IdentityResolver for IdentityResolverImpl {
    async fn resolve_sender(
        &self,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedIdentity> {
        let Some(binding) =
            get_channel_user_binding_by_user_id(&self.pool, inst.id, &msg.source.sender_id).await?
        else {
            return Err(ResolverError::SenderUnbound.into());
        };
        if get_member_by_user_and_workspace(&self.pool, binding.cordy_user_id, inst.workspace_id)
            .await?
            .is_none()
        {
            return Err(ResolverError::SenderNotMember.into());
        }
        Ok(ResolvedIdentity {
            user_id: binding.cordy_user_id,
        })
    }
}

struct DeduperImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl Deduper for DeduperImpl {
    async fn claim(&self, installation_id: Uuid, message_id: &str) -> anyhow::Result<Uuid> {
        let Some(row) =
            claim_channel_inbound_dedup(&self.pool, installation_id, message_id).await?
        else {
            return Err(ResolverError::Duplicate.into());
        };
        Ok(row.claim_token)
    }

    async fn mark(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid) {
        let _ = mark_channel_inbound_dedup_processed(
            &self.pool,
            installation_id,
            message_id,
            claim_token,
        )
        .await;
    }

    async fn release(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid) {
        let _ = release_channel_inbound_dedup(&self.pool, installation_id, message_id, claim_token)
            .await;
    }
}

struct SessionBinderImpl {
    session: ChatSession,
}

#[async_trait::async_trait]
impl SessionBinder for SessionBinderImpl {
    async fn ensure_session(&self, p: EnsureSessionParams) -> anyhow::Result<Uuid> {
        let (binding_key, config, _) = telegram_session_routing(&p.message);
        self.session
            .ensure_session(&EnsureSessionInput {
                workspace_id: p.installation.workspace_id,
                agent_id: p.installation.agent_id,
                installation_id: p.installation.id,
                sender: p.sender,
                binding_key,
                binding_config: Some(config),
                chat_type: p.message.source.chat_type.clone(),
            })
            .await
    }

    async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()> {
        self.session.mark_pending_fresh(session_id).await
    }

    async fn append_message(&self, p: EngineAppendParams) -> anyhow::Result<AppendResult> {
        // Route-revision fencing is a Lark-side concern; Telegram has no
        // multi-agent route to invalidate, so ClaimLost can only come from
        // the dedup token itself.
        let input = AppendInput {
            session_id: p.session_id,
            sender: p.sender,
            installation_id: p.installation_id,
            body: p.message.text.clone(),
            command_text: p.message.command_text.clone(),
            message_id: p.message.message_id.clone(),
            thread_id: p.message.source.thread_id.clone(),
            claim_token: p.claim_token,
            media_pending_seconds: p.media_pending_seconds,
            force_fresh: p.message.force_fresh,
        };
        self.session.append_user_message(&input).await
    }

    async fn bind_media(&self, p: EngineBindMediaParams) -> anyhow::Result<()> {
        self.session
            .bind_media_refs(&BindMediaInput {
                message_id: p.message_id,
                session_id: p.session_id,
                workspace_id: p.workspace_id,
                sender: p.sender,
                issue_id: p.issue_id,
                issue_description_base: p.issue_description_base,
                issue_command_text: p.issue_command_text,
                body: p.body,
                media_refs: p.media_refs,
            })
            .await
    }
}

struct AuditorImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl cordy_channel_engine::resolvers::Auditor for AuditorImpl {
    async fn record_drop(&self, inst_id: Uuid, msg: &InboundMessage, reason: &DropReason) {
        let event_type = decode_telegram_raw(msg)
            .map(|raw| raw.event_type)
            .unwrap_or_default();
        let result = record_channel_inbound_drop(
            &self.pool,
            crate::TYPE_TELEGRAM,
            &event_type,
            &reason.0,
            (!inst_id.is_nil()).then_some(inst_id),
            opt_str(&msg.source.chat_id),
            opt_str(&msg.event_id),
            opt_str(&msg.message_id),
            cordy_db::dbid::new_v7(),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, "telegram audit: record drop failed");
        }
    }
}

fn opt_str(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

/// Builds the full Telegram ResolverSet over a pool and an optional replier.
pub fn new_telegram_resolver_set(
    pool: PgPool,
    replier: Option<Arc<dyn cordy_channel_engine::resolvers::OutboundReplier>>,
    typing: Option<Arc<dyn TypingNotifier>>,
    media: Option<Arc<dyn cordy_channel_engine::resolvers::MediaResolver>>,
) -> ResolverSet {
    let session = ChatSession::new(
        pool.clone(),
        cordy_channel::Type(crate::TYPE_TELEGRAM.to_string()),
        SessionTitles {
            group: "Telegram group".into(),
            direct: "Telegram direct message".into(),
            fallback: "Telegram chat".into(),
        },
    );
    ResolverSet {
        installation: Some(Arc::new(InstallationResolverImpl { pool: pool.clone() })),
        identity: Some(Arc::new(IdentityResolverImpl { pool: pool.clone() })),
        validated: None,
        dedup: Some(Arc::new(DeduperImpl { pool: pool.clone() })),
        session: Some(Arc::new(SessionBinderImpl { session })),
        media,
        audit: Some(Arc::new(AuditorImpl { pool })),
        replier,
        typing,
        origin_type: ORIGIN_TELEGRAM_CHAT.to_string(),
    }
}
