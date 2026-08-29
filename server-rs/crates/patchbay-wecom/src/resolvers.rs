//! Production resolver adapters between normalized channel messages and the
//! WeCom smart-bot runtime.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use patchbay_channel::InboundMessage;
use patchbay_channel_engine::resolvers::{
    AppendParams, AppendResult, Auditor, BindMediaParams, Deduper, DropReason, EnsureSessionParams,
    IdentityResolver, InstallationResolver, MediaResolver, OutboundReplier, ResolvedIdentity,
    ResolvedInstallation, ResolverError, ResolverSet, SessionBinder,
};
use patchbay_channel_engine::session::{
    AppendInput, BindMediaInput, ChatSession, EnsureSessionInput, SessionTitles,
};
use patchbay_db::queries::channel::{
    claim_channel_inbound_dedup, get_channel_installation_by_app_id,
    get_channel_user_binding_by_user_id, mark_channel_inbound_dedup_processed,
    record_channel_inbound_drop, release_channel_inbound_dedup,
};
use patchbay_db::queries::member::get_member_by_user_and_workspace;

pub const ORIGIN_WECOM_CHAT: &str = "wecom_chat";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WecomBindingConfig {
    pub chat_id: String,
    pub chat_type: i64,
}

pub fn wecom_session_routing(msg: &InboundMessage) -> (String, serde_json::Value) {
    let chat_id = msg.source.chat_id.clone();
    let config = serde_json::to_value(WecomBindingConfig {
        chat_id: chat_id.clone(),
        chat_type: crate::ws_frame::aibot_chat_type_from_channel(&msg.source.chat_type),
    })
    .unwrap_or_else(|_| serde_json::json!({}));
    (chat_id, config)
}

struct InstallationResolverImpl {
    pool: PgPool,
}

#[async_trait]
impl InstallationResolver for InstallationResolverImpl {
    async fn resolve_installation(
        &self,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        let raw = crate::ws_frame::wecom_msg_from_raw(msg)?;
        if raw.bot_id.is_empty() {
            return Err(ResolverError::InstallationNotFound.into());
        }
        let Some(inst) =
            get_channel_installation_by_app_id(&self.pool, crate::TYPE_WECOM, &raw.bot_id).await?
        else {
            return Err(ResolverError::InstallationNotFound.into());
        };
        Ok(ResolvedInstallation {
            id: inst.id,
            workspace_id: inst.workspace_id,
            agent_id: inst.agent_id.unwrap_or_default(),
            route_revision: 0,
            installer_user_id: inst.installer_user_id,
            active: inst.status == crate::types::INSTALLATION_ACTIVE,
            platform: Arc::new(inst),
        })
    }
}

struct IdentityResolverImpl {
    pool: PgPool,
}

#[async_trait]
impl IdentityResolver for IdentityResolverImpl {
    async fn resolve_sender(
        &self,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedIdentity> {
        if msg.source.sender_id.trim().is_empty() {
            return Err(ResolverError::SenderUnbound.into());
        }
        let Some(binding) =
            get_channel_user_binding_by_user_id(&self.pool, inst.id, &msg.source.sender_id).await?
        else {
            return Err(ResolverError::SenderUnbound.into());
        };
        if get_member_by_user_and_workspace(&self.pool, binding.patchbay_user_id, inst.workspace_id)
            .await?
            .is_none()
        {
            return Err(ResolverError::SenderNotMember.into());
        }
        Ok(ResolvedIdentity {
            user_id: binding.patchbay_user_id,
        })
    }
}

struct DeduperImpl {
    pool: PgPool,
}

#[async_trait]
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

#[async_trait]
impl SessionBinder for SessionBinderImpl {
    fn binding_key(&self, msg: &InboundMessage) -> String {
        wecom_session_routing(msg).0
    }

    async fn ensure_session(&self, p: EnsureSessionParams) -> anyhow::Result<Uuid> {
        let (binding_key, config) = wecom_session_routing(&p.message);
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

    async fn append_message(&self, p: AppendParams) -> anyhow::Result<AppendResult> {
        let command_text = if p.message.command_text.is_empty() {
            p.message.text.clone()
        } else {
            p.message.command_text.clone()
        };
        self.session
            .append_user_message(&AppendInput {
                session_id: p.session_id,
                sender: p.sender,
                installation_id: p.installation_id,
                body: p.message.text.clone(),
                command_text,
                message_id: p.message.message_id.clone(),
                thread_id: String::new(),
                claim_token: p.claim_token,
                media_pending_seconds: p.media_pending_seconds,
                force_fresh: p.message.force_fresh,
            })
            .await
    }

    async fn bind_media(&self, p: BindMediaParams) -> anyhow::Result<()> {
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

#[async_trait]
impl Auditor for AuditorImpl {
    async fn record_drop(&self, inst_id: Uuid, msg: &InboundMessage, reason: &DropReason) {
        let event_type = crate::ws_frame::wecom_msg_from_raw(msg)
            .map(|raw| raw.msg_type)
            .unwrap_or_default();
        if let Err(error) = record_channel_inbound_drop(
            &self.pool,
            crate::TYPE_WECOM,
            &event_type,
            &reason.0,
            (!inst_id.is_nil()).then_some(inst_id),
            opt_str(&msg.source.chat_id),
            opt_str(&msg.event_id),
            opt_str(&msg.message_id),
            patchbay_db::dbid::new_v7(),
        )
        .await
        {
            tracing::warn!(%error, "wecom audit: record drop failed");
        }
    }
}

fn opt_str(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

pub fn new_wecom_resolver_set(
    pool: PgPool,
    replier: Option<Arc<dyn OutboundReplier>>,
    media: Option<Arc<dyn MediaResolver>>,
) -> ResolverSet {
    let session = ChatSession::new(
        pool.clone(),
        crate::type_wecom(),
        SessionTitles {
            group: "WeCom group".into(),
            direct: "WeCom direct message".into(),
            fallback: "WeCom chat".into(),
        },
    );
    ResolverSet {
        installation: Some(Arc::new(InstallationResolverImpl { pool: pool.clone() })),
        identity: Some(Arc::new(IdentityResolverImpl { pool: pool.clone() })),
        validated: None,
        dedup: Some(Arc::new(DeduperImpl { pool: pool.clone() })),
        session: Some(Arc::new(SessionBinderImpl { session })),
        media,
        audit: Some(Arc::new(AuditorImpl { pool: pool.clone() })),
        replier,
        typing: None,
        hub: Some(Arc::new(
            patchbay_channel_engine::hub::PostgresHubRouter::new(pool.clone()),
        )),
        origin_type: ORIGIN_WECOM_CHAT.to_string(),
    }
}
