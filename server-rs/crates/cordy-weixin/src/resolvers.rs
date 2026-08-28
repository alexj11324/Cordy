use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use cordy_channel::InboundMessage;
use cordy_channel_engine::resolvers::{
    AppendParams, AppendResult, BindMediaParams, Deduper, DropReason, EnsureSessionParams,
    IdentityResolver, InstallationResolver, ResolvedIdentity, ResolvedInstallation, ResolverError,
    ResolverSet, SessionBinder,
};
use cordy_channel_engine::session::{
    AppendInput, BindMediaInput, ChatSession, EnsureSessionInput, SessionTitles,
};
use cordy_db::queries::channel::{
    claim_channel_inbound_dedup, get_channel_installation_by_app_id,
    get_channel_user_binding_by_user_id, mark_channel_inbound_dedup_processed,
    merge_channel_chat_session_binding_config, record_channel_inbound_drop,
    release_channel_inbound_dedup,
};
use cordy_db::queries::member::get_member_by_user_and_workspace;

use crate::inbound::WeixinRawEvent;

pub const ORIGIN_WEIXIN_CHAT: &str = "weixin_chat";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WeixinBindingConfig {
    pub user_id: String,
    pub context_token_encrypted: String,
}

pub type ContextSealer = dyn Fn(&[u8]) -> anyhow::Result<Vec<u8>> + Send + Sync;

fn raw(message: &InboundMessage) -> anyhow::Result<WeixinRawEvent> {
    serde_json::from_value(message.raw.clone())
        .map_err(|error| anyhow::anyhow!("decode weixin inbound raw: {error}"))
}

struct InstallationResolverImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl InstallationResolver for InstallationResolverImpl {
    async fn resolve_installation(
        &self,
        message: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        let raw = raw(message)?;
        let Some(installation) =
            get_channel_installation_by_app_id(&self.pool, crate::TYPE_WEIXIN, &raw.bot_id).await?
        else {
            return Err(ResolverError::InstallationNotFound.into());
        };
        Ok(ResolvedInstallation {
            id: installation.id,
            workspace_id: installation.workspace_id,
            agent_id: installation.agent_id,
            route_revision: 0,
            installer_user_id: installation.installer_user_id,
            active: installation.status == "active",
            platform: Arc::new(installation),
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
        installation: &ResolvedInstallation,
        message: &InboundMessage,
    ) -> anyhow::Result<ResolvedIdentity> {
        let Some(binding) = get_channel_user_binding_by_user_id(
            &self.pool,
            installation.id,
            &message.source.sender_id,
        )
        .await?
        else {
            return Err(ResolverError::SenderUnbound.into());
        };
        if get_member_by_user_and_workspace(
            &self.pool,
            binding.cordy_user_id,
            installation.workspace_id,
        )
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
        claim_channel_inbound_dedup(&self.pool, installation_id, message_id)
            .await?
            .map(|row| row.claim_token)
            .ok_or_else(|| ResolverError::Duplicate.into())
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
    pool: PgPool,
    session: ChatSession,
    seal: Arc<ContextSealer>,
}

#[async_trait::async_trait]
impl SessionBinder for SessionBinderImpl {
    async fn ensure_session(&self, params: EnsureSessionParams) -> anyhow::Result<Uuid> {
        let raw = raw(&params.message)?;
        let binding_key = params.message.source.chat_id.clone();
        use base64::Engine as _;
        let context_token_encrypted = base64::engine::general_purpose::STANDARD
            .encode((self.seal)(raw.context_token.as_bytes())?);
        let config = serde_json::to_value(WeixinBindingConfig {
            user_id: params.message.source.sender_id.clone(),
            context_token_encrypted,
        })?;
        let session_id = self
            .session
            .ensure_session(&EnsureSessionInput {
                workspace_id: params.installation.workspace_id,
                agent_id: params.installation.agent_id,
                installation_id: params.installation.id,
                sender: params.sender,
                binding_key: binding_key.clone(),
                binding_config: Some(config.clone()),
                chat_type: params.message.source.chat_type.clone(),
            })
            .await?;
        // `ensure_session` intentionally avoids writes on the hot existing
        // path. iLink's context token changes over time, so merge the latest
        // value after every accepted inbound message for delayed ChatDone.
        merge_channel_chat_session_binding_config(
            &self.pool,
            params.installation.id,
            &binding_key,
            &config,
        )
        .await?;
        Ok(session_id)
    }

    async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()> {
        self.session.mark_pending_fresh(session_id).await
    }

    async fn append_message(&self, params: AppendParams) -> anyhow::Result<AppendResult> {
        self.session
            .append_user_message(&AppendInput {
                session_id: params.session_id,
                sender: params.sender,
                installation_id: params.installation_id,
                body: params.message.text.clone(),
                command_text: params.message.command_text.clone(),
                message_id: params.message.message_id.clone(),
                thread_id: String::new(),
                claim_token: params.claim_token,
                media_pending_seconds: params.media_pending_seconds,
                force_fresh: params.message.force_fresh,
            })
            .await
    }

    async fn bind_media(&self, params: BindMediaParams) -> anyhow::Result<()> {
        self.session
            .bind_media_refs(&BindMediaInput {
                message_id: params.message_id,
                session_id: params.session_id,
                workspace_id: params.workspace_id,
                sender: params.sender,
                issue_id: params.issue_id,
                issue_description_base: params.issue_description_base,
                issue_command_text: params.issue_command_text,
                body: params.body,
                media_refs: params.media_refs,
            })
            .await
    }
}

struct AuditorImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl cordy_channel_engine::resolvers::Auditor for AuditorImpl {
    async fn record_drop(
        &self,
        installation_id: Uuid,
        message: &InboundMessage,
        reason: &DropReason,
    ) {
        let _ = record_channel_inbound_drop(
            &self.pool,
            crate::TYPE_WEIXIN,
            "message",
            &reason.0,
            (!installation_id.is_nil()).then_some(installation_id),
            Some(&message.source.chat_id),
            Some(&message.event_id),
            Some(&message.message_id),
            cordy_db::dbid::new_v7(),
        )
        .await;
    }
}

pub fn resolver_set(
    pool: PgPool,
    replier: Option<Arc<dyn cordy_channel_engine::resolvers::OutboundReplier>>,
    seal: Arc<ContextSealer>,
) -> ResolverSet {
    let session = ChatSession::new(
        pool.clone(),
        cordy_channel::Type(crate::TYPE_WEIXIN.to_string()),
        SessionTitles {
            group: "WeChat group".into(),
            direct: "WeChat direct message".into(),
            fallback: "WeChat chat".into(),
        },
    );
    ResolverSet {
        installation: Some(Arc::new(InstallationResolverImpl { pool: pool.clone() })),
        identity: Some(Arc::new(IdentityResolverImpl { pool: pool.clone() })),
        validated: None,
        dedup: Some(Arc::new(DeduperImpl { pool: pool.clone() })),
        session: Some(Arc::new(SessionBinderImpl {
            pool: pool.clone(),
            session,
            seal,
        })),
        media: None,
        audit: Some(Arc::new(AuditorImpl { pool })),
        replier,
        typing: None,
        origin_type: ORIGIN_WEIXIN_CHAT.to_string(),
    }
}
