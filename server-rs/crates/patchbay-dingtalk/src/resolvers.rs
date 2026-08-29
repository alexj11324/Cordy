//! Port of `resolvers.go`: the DingTalk ResolverSet — the platform-specific
//! seams the channel-agnostic engine Router runs the inbound pipeline through.
//! It is built entirely on the generic channel_* queries plus the shared
//! engine.ChatSession, mirroring the Feishu and Slack ResolverSets.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use patchbay_channel::{ChatType, InboundMessage};
use patchbay_channel_engine::resolvers::{
    AppendParams, AppendResult, Auditor, BindMediaParams, Deduper, DropReason, EnsureSessionParams,
    IdentityResolver, InstallationResolver, ResolvedIdentity, ResolvedInstallation, ResolverError,
    ResolverSet, SessionBinder, ValidatedInboundResolver,
};
use patchbay_channel_engine::session::{
    AppendFence, AppendInput, ChatSession, EnsureSessionInput, SessionTitles,
};
use patchbay_db::models::ChannelChatSessionBinding;
use patchbay_db::queries::channel::{
    claim_channel_inbound_dedup, get_channel_installation_by_app_id,
    get_channel_user_binding_by_user_id, mark_channel_inbound_dedup_processed,
    record_channel_inbound_drop, release_channel_inbound_dedup,
};
use patchbay_db::queries::dingtalk::{
    delete_ding_talk_stale_group_chat_binding, ding_talk_group_route_matches_agent,
    discover_ding_talk_group_route, lock_ding_talk_group_route_for_append,
};

use crate::channel_type;
use crate::inbound::{DingtalkRawEvent, CONV_TYPE_GROUP, CONV_TYPE_P2P};
use crate::outbound_send::SendTarget;
use crate::ORIGIN_DINGTALK_CHAT;

/// Assembles the DingTalk ResolverSet over the pool. The replier delivers the
/// outbound binding-prompt / status / issue-created notices; pass None to
/// disable them. The classic robot send API exposes no per-message reaction, so
/// the ack notifier stands in for a typing indicator (a "working on it"
/// message on ingest); pass None to disable it. Media is optional: when
/// configured it uses the shared MediaResolver and intent-ledger pipeline.
pub fn new_dingtalk_resolver_set(
    pool: sqlx::PgPool,
    replier: Option<Arc<dyn patchbay_channel_engine::resolvers::OutboundReplier>>,
    ack: Option<Arc<dyn patchbay_channel_engine::resolvers::TypingNotifier>>,
    media: Option<Arc<dyn patchbay_channel_engine::resolvers::MediaResolver>>,
) -> ResolverSet {
    let session = ChatSession::new(
        pool.clone(),
        channel_type(),
        SessionTitles {
            group: "DingTalk group".to_string(),
            direct: "DingTalk direct message".to_string(),
            fallback: "DingTalk chat".to_string(),
        },
    );
    ResolverSet {
        installation: Some(Arc::new(InstallationResolverImpl { pool: pool.clone() })),
        identity: Some(Arc::new(IdentityResolverImpl { pool: pool.clone() })),
        validated: Some(Arc::new(ValidatedInboundResolverImpl {
            pool: pool.clone(),
        })),
        dedup: Some(Arc::new(DeduperImpl { pool: pool.clone() })),
        session: Some(Arc::new(SessionBinderImpl {
            pool: pool.clone(),
            session,
        })),
        audit: Some(Arc::new(AuditorImpl { pool: pool.clone() })),
        replier,
        typing: ack,
        media,
        hub: Some(Arc::new(patchbay_channel_engine::hub::PostgresHubRouter::new(
            pool.clone(),
        ))),
        origin_type: ORIGIN_DINGTALK_CHAT.to_string(),
    }
}

/// The opaque outbound routing persisted on the chat binding's config: enough
/// to address a proactive reply back into the originating conversation.
/// staff_id is the lone recipient of a 1:1 chat; for a group it is empty (the
/// group is addressed by its conversation id).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DingtalkBindingConfig {
    #[serde(rename = "conversation_type", default)]
    conversation_type: String,
    #[serde(rename = "conversation_id", default)]
    conversation_id: String,
    #[serde(rename = "staff_id", default, skip_serializing_if = "String::is_empty")]
    staff_id: String,
    #[serde(rename = "agent_id", default, skip_serializing_if = "String::is_empty")]
    agent_id: String,
}

/// Derives the session-isolation key and the outbound routing config from one
/// inbound message. DingTalk has no threads, so a conversation (1:1 or group)
/// is one continuous session keyed by its conversation id; the config carries
/// everything the outbound path needs to send back.
fn dingtalk_session_routing(msg: &InboundMessage, agent_id: Uuid) -> (String, serde_json::Value) {
    let chat_id = msg.source.chat_id.clone();
    let mut cfg = DingtalkBindingConfig {
        conversation_type: CONV_TYPE_GROUP.to_string(),
        conversation_id: chat_id.clone(),
        staff_id: String::new(),
        agent_id: agent_id.to_string(),
    };
    if msg.source.chat_type == ChatType::p2p() {
        cfg.conversation_type = CONV_TYPE_P2P.to_string();
        cfg.staff_id = msg.source.sender_id.clone();
    }
    (
        chat_id,
        serde_json::to_value(cfg).unwrap_or(serde_json::Value::Null),
    )
}

/// Recovers the send target from a chat binding's config, falling back to the
/// channel_chat_id when the config is missing or unparsable.
pub(crate) fn outbound_target(b: &ChannelChatSessionBinding) -> SendTarget {
    let mut target = SendTarget {
        conversation_type: CONV_TYPE_GROUP.to_string(),
        conversation_id: b.channel_chat_id.clone(),
        staff_id: String::new(),
    };
    if !b.config.is_null() {
        if let Ok(cfg) = serde_json::from_value::<DingtalkBindingConfig>(b.config.clone()) {
            if !cfg.conversation_type.is_empty() {
                target.conversation_type = cfg.conversation_type;
            }
            if !cfg.conversation_id.is_empty() {
                target.conversation_id = cfg.conversation_id;
            }
            target.staff_id = cfg.staff_id;
        }
    }
    target
}

pub(crate) fn decode_dingtalk_raw(msg: &InboundMessage) -> anyhow::Result<DingtalkRawEvent> {
    if msg.raw.is_null() {
        anyhow::bail!("dingtalk: inbound message Raw is empty");
    }
    serde_json::from_value(msg.raw.clone())
        .map_err(|e| anyhow::anyhow!("decode dingtalk inbound raw: {e}"))
}

// ---- installation routing ----

struct InstallationResolverImpl {
    pool: sqlx::PgPool,
}

#[async_trait]
impl InstallationResolver for InstallationResolverImpl {
    /// Routes by the AppKey the receiving connection stamped into the envelope.
    /// Each installation has its own Stream connection, so the stamped AppKey
    /// uniquely identifies the installation (the DingTalk callback itself
    /// carries no robot code).
    async fn resolve_installation(
        &self,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        let raw = decode_dingtalk_raw(msg)?;
        let inst =
            get_channel_installation_by_app_id(&self.pool, crate::TYPE_DINGTALK, &raw.app_id)
                .await?;
        let Some(inst) = inst else {
            return Err(ResolverError::InstallationNotFound.into());
        };
        Ok(resolved_installation(&inst, inst.agent_id.unwrap_or_default()))
    }
}

// ---- validated inbound discovery / final routing ----

struct ValidatedInboundResolverImpl {
    pool: sqlx::PgPool,
}

#[async_trait]
impl ValidatedInboundResolver for ValidatedInboundResolverImpl {
    async fn resolve_validated_inbound(
        &self,
        mut inst: ResolvedInstallation,
        _identity: &ResolvedIdentity,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        if inst.agent_id.is_nil() {
            return Ok(inst);
        }
        if msg.source.chat_type != ChatType::group() {
            return Ok(inst);
        }
        let raw = decode_dingtalk_raw(msg)?;
        let row = discover_ding_talk_group_route(
            &self.pool,
            inst.workspace_id,
            inst.id,
            &msg.source.chat_id,
            &raw.conversation_title,
        )
        .await?;
        let Some(row) = row else {
            anyhow::bail!("discover dingtalk group route: no row returned");
        };
        match row.agent_id {
            Some(agent_id) => inst.agent_id = agent_id,
            None => anyhow::bail!("discover dingtalk group route: null agent"),
        }
        inst.route_revision = row.revision;
        if !row.agent_active {
            return Err(ResolverError::TargetAgentArchived.into());
        }
        Ok(inst)
    }
}

pub(crate) fn resolved_installation(
    inst: &patchbay_db::models::ChannelInstallation,
    agent_id: Uuid,
) -> ResolvedInstallation {
    ResolvedInstallation {
        id: inst.id,
        workspace_id: inst.workspace_id,
        agent_id,
        route_revision: 0,
        installer_user_id: inst.installer_user_id,
        active: inst.status == "active",
        platform: Arc::new(inst.clone()),
    }
}

// ---- identity ----

struct IdentityResolverImpl {
    pool: sqlx::PgPool,
}

#[async_trait]
impl IdentityResolver for IdentityResolverImpl {
    async fn resolve_sender(
        &self,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedIdentity> {
        let binding =
            get_channel_user_binding_by_user_id(&self.pool, inst.id, &msg.source.sender_id).await?;
        let Some(binding) = binding else {
            return Err(ResolverError::SenderUnbound.into());
        };
        // Binding existence no longer proves membership (no FK); re-check.
        let member = patchbay_db::queries::member::get_member_by_user_and_workspace(
            &self.pool,
            binding.patchbay_user_id,
            inst.workspace_id,
        )
        .await?;
        if member.is_none() {
            return Err(ResolverError::SenderNotMember.into());
        }
        Ok(ResolvedIdentity {
            user_id: binding.patchbay_user_id,
        })
    }
}

// ---- dedup ----

struct DeduperImpl {
    pool: sqlx::PgPool,
}

#[async_trait]
impl Deduper for DeduperImpl {
    async fn claim(&self, installation_id: Uuid, message_id: &str) -> anyhow::Result<Uuid> {
        let claim = claim_channel_inbound_dedup(&self.pool, installation_id, message_id).await?;
        let Some(claim) = claim else {
            return Err(ResolverError::Duplicate.into());
        };
        Ok(claim.claim_token)
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

// ---- session bind / append ----

struct SessionBinderImpl {
    pool: sqlx::PgPool,
    session: ChatSession,
}

struct GroupRouteFence {
    installation_id: Uuid,
    chat_id: String,
    agent_id: Uuid,
    route_revision: i64,
}

#[async_trait]
impl AppendFence for GroupRouteFence {
    async fn before_write(&self, tx: &mut sqlx::PgConnection) -> anyhow::Result<()> {
        let locked = lock_ding_talk_group_route_for_append(
            tx,
            self.installation_id,
            &self.chat_id,
            self.agent_id,
            self.route_revision,
        )
        .await
        .map_err(|e| anyhow::anyhow!("lock dingtalk group route for append: {e:#}"))?;
        if locked.is_none() {
            return Err(ResolverError::RouteChanged.into());
        }
        Ok(())
    }
}

#[async_trait]
impl SessionBinder for SessionBinderImpl {
    fn binding_key(&self, msg: &InboundMessage) -> String {
        dingtalk_session_routing(msg, Uuid::nil()).0
    }

    async fn ensure_session(&self, p: EnsureSessionParams) -> anyhow::Result<Uuid> {
        let (binding_key, config) = dingtalk_session_routing(&p.message, p.installation.agent_id);
        let input = EnsureSessionInput {
            workspace_id: p.installation.workspace_id,
            agent_id: p.installation.agent_id,
            installation_id: p.installation.id,
            sender: p.sender,
            binding_key: binding_key.clone(),
            binding_config: Some(config),
            chat_type: p.message.source.chat_type.clone(),
        };
        if p.message.source.chat_type != ChatType::group() || p.installation.route_revision == 0 {
            return self.session.ensure_session(&input).await;
        }

        for _attempt in 0..3 {
            let matches = ding_talk_group_route_matches_agent(
                &self.pool,
                p.installation.id,
                &binding_key,
                p.installation.agent_id,
                p.installation.route_revision,
            )
            .await
            .map_err(|e| anyhow::anyhow!("verify dingtalk group route: {e:#}"))?
            .unwrap_or(false);
            if !matches {
                return Err(ResolverError::RouteChanged.into());
            }
            delete_ding_talk_stale_group_chat_binding(
                &self.pool,
                p.installation.id,
                &binding_key,
                p.installation.agent_id,
            )
            .await
            .map_err(|e| anyhow::anyhow!("retire stale dingtalk group session: {e:#}"))?;

            let session_id = self.session.ensure_session(&input).await?;
            let matches = ding_talk_group_route_matches_agent(
                &self.pool,
                p.installation.id,
                &binding_key,
                p.installation.agent_id,
                p.installation.route_revision,
            )
            .await
            .map_err(|e| anyhow::anyhow!("recheck dingtalk group route: {e:#}"))?
            .unwrap_or(false);
            if !matches {
                return Err(ResolverError::RouteChanged.into());
            }
            let retired = delete_ding_talk_stale_group_chat_binding(
                &self.pool,
                p.installation.id,
                &binding_key,
                p.installation.agent_id,
            )
            .await
            .map_err(|e| anyhow::anyhow!("recheck dingtalk group session: {e:#}"))?
            .unwrap_or(0);
            if retired == 0 {
                return Ok(session_id);
            }
        }
        Err(ResolverError::RouteChanged.into())
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
        let input = AppendInput {
            session_id: p.session_id,
            sender: p.sender,
            installation_id: p.installation_id,
            body: p.message.text.clone(),
            command_text,
            message_id: p.message.message_id.clone(),
            thread_id: p.message.source.thread_id.clone(),
            claim_token: p.claim_token,
            media_pending_seconds: p.media_pending_seconds,
            force_fresh: p.message.force_fresh,
        };

        let fence = (p.message.source.chat_type == ChatType::group()
            && p.route_revision != 0)
            .then(|| GroupRouteFence {
            installation_id: p.installation_id,
            chat_id: p.message.source.chat_id.clone(),
            agent_id: p.agent_id,
            route_revision: p.route_revision,
        });
        self.session
            .append_user_message_fenced(&input, fence.as_ref().map(|f| f as &dyn AppendFence))
            .await
    }

    async fn bind_media(&self, p: BindMediaParams) -> anyhow::Result<()> {
        self.session
            .bind_media_refs(&patchbay_channel_engine::session::BindMediaInput {
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

// ---- audit ----

struct AuditorImpl {
    pool: sqlx::PgPool,
}

#[async_trait]
impl Auditor for AuditorImpl {
    async fn record_drop(&self, inst_id: Uuid, msg: &InboundMessage, reason: &DropReason) {
        let result = record_channel_inbound_drop(
            &self.pool,
            crate::TYPE_DINGTALK,
            "message",
            &reason.0,
            Some(inst_id),
            opt_str(&msg.source.chat_id),
            opt_str(&msg.event_id),
            opt_str(&msg.message_id),
            patchbay_db::dbid::new_v7(),
        )
        .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "dingtalk audit: record drop failed");
        }
    }
}

fn opt_str(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}
