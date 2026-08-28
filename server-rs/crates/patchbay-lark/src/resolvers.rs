//! The Feishu ResolverSet.
//!
//! The platform-specific implementations the channel-agnostic engine Router
//! runs the inbound pipeline through. Each resolver translates between the
//! engine's normalized `patchbay_channel::InboundMessage` / engine types and the
//! Feishu store / services. Platform-specific fields the normalized envelope
//! does not carry (app_id, event_type, create time) are read from the original
//! [`InboundMessage`] that the Feishu channel stashes in
//! `channel.InboundMessage.raw` — the documented adapter boundary (the core
//! never reads Raw).

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::InboundMessage as ChannelMessage;
use patchbay_channel_engine::resolvers::{
    AppendParams, Auditor, BindMediaParams, Deduper, DropReason, EnsureSessionParams,
    IdentityResolver, InstallationResolver, MediaResolver, OutboundReplier, ResolvedIdentity,
    ResolvedInstallation, ResolverError, Result as EngineResult, SessionBinder, TypingNotifier,
};

use crate::channel_store::ChannelStore;
use crate::chat::AuditDropParams;
use crate::client::ApiClient;
use crate::feishu_types::InboundMessage;
use crate::installation::{installation_credentials_for, CredentialsResolver};
use crate::store::Installation;
use crate::types::{ChatId, ChatType, OpenId};

/// The issue.origin_type label written for issues created via the Feishu
/// /issue command. Kept as "lark_chat" (unchanged from the pre-cutover
/// dispatcher) so analytics classification does not shift.
pub const ORIGIN_FEISHU_CHAT: &str = "lark_chat";

/// Decodes the original Feishu [`InboundMessage`] the Feishu channel stashed
/// in `channel.InboundMessage.raw`.
pub fn lark_msg_from_raw(msg: &ChannelMessage) -> anyhow::Result<InboundMessage> {
    if msg.raw.is_null() {
        anyhow::bail!("lark: inbound message Raw is empty");
    }
    let lm: InboundMessage = serde_json::from_value(msg.raw.clone())
        .map_err(|e| anyhow::anyhow!("decode feishu inbound raw: {e}"))?;
    Ok(lm)
}

/// The opaque outbound routing persisted on the chat binding's config when the
/// binding key is a composite (Lark topic): the real chat id lives here so the
/// outbound path can post back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LarkBindingConfig {
    #[serde(default, rename = "chat_id")]
    pub chat_id: String,
}

/// Derives the session-isolation key (stored as channel_chat_id) and the
/// outbound config from one inbound Feishu message. A p2p or plain group chat
/// is one continuous session per chat, so the key is the chat id and the key
/// alone routes outbound (no config). A message inside a Lark topic (话题,
/// thread_id present) is isolated by topic — key = "chat:thread" — so two
/// @bot topics in one group are two sessions (the same model as Slack's
/// channel:threadRoot; see engine.EnsureSessionInput). Pure function so the
/// isolation contract is unit-tested without a DB.
pub fn lark_session_routing(msg: &ChannelMessage) -> (String, Option<serde_json::Value>) {
    let chat_id = msg.source.chat_id.clone();
    if msg.source.chat_type.0 != ChatType::group().0 || msg.source.thread_id.is_empty() {
        return (chat_id, None);
    }
    let cfg = serde_json::to_value(LarkBindingConfig {
        chat_id: chat_id.clone(),
    })
    .ok();
    (format!("{chat_id}:{}", msg.source.thread_id), cfg)
}

/// Assembles the Feishu ResolverSet from the store, the shared session
/// service, audit logger, and (optional) outbound replier + typing indicator +
/// media resolver. Feishu is just another consumer of the channel-agnostic
/// engine ChatSession — there is no Feishu-specific session implementation.
#[allow(clippy::too_many_arguments)]
pub fn new_feishu_resolver_set(
    store: Arc<ChannelStore>,
    session: Arc<dyn FeishuChatSession>,
    audit: Arc<dyn crate::chat::AuditLogger>,
    replier: Option<Arc<dyn OutcomeReplier>>,
    typing: Option<Arc<TypingIndicatorManager>>,
    media: Option<Arc<dyn MediaResolver>>,
) -> patchbay_channel_engine::resolvers::ResolverSet {
    let mut set = patchbay_channel_engine::resolvers::ResolverSet {
        installation: Some(Arc::new(FeishuInstallationResolver {
            store: store.clone(),
        })),
        identity: Some(Arc::new(FeishuIdentityResolver {
            store: store.clone(),
        })),
        dedup: Some(Arc::new(FeishuDeduper(store))),
        session: Some(Arc::new(FeishuSessionBinder { session })),
        audit: Some(Arc::new(FeishuAuditor { audit })),
        origin_type: ORIGIN_FEISHU_CHAT.to_string(),
        ..Default::default()
    };
    if let Some(replier) = replier {
        set.replier = Some(Arc::new(FeishuOutboundReplier { replier }));
    }
    if let Some(typing) = typing {
        set.typing = Some(Arc::new(FeishuTypingNotifier(typing)));
    }
    set.media = media;
    set
}

/// The channel-agnostic session input types the Feishu binder drives. Declared
/// as a trait so the (platform-specific) param mapping can be unit-tested with
/// a fake; `engine.ChatSession` is the production value.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait FeishuChatSession: Send + Sync {
    async fn ensure_session(
        &self,
        workspace_id: Uuid,
        agent_id: Uuid,
        installation_id: Uuid,
        sender: Uuid,
        binding_key: String,
        binding_config: Option<serde_json::Value>,
        chat_type: ChatType,
    ) -> anyhow::Result<Uuid>;
    async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()>;
    async fn append_user_message(
        &self,
        input: &patchbay_channel_engine::session::AppendInput,
    ) -> anyhow::Result<patchbay_channel_engine::resolvers::AppendResult>;
    async fn bind_media_refs(
        &self,
        input: &patchbay_channel_engine::session::BindMediaInput,
    ) -> anyhow::Result<()>;
}

#[async_trait]
impl FeishuChatSession for patchbay_channel_engine::session::ChatSession {
    async fn ensure_session(
        &self,
        workspace_id: Uuid,
        agent_id: Uuid,
        installation_id: Uuid,
        sender: Uuid,
        binding_key: String,
        binding_config: Option<serde_json::Value>,
        chat_type: ChatType,
    ) -> anyhow::Result<Uuid> {
        patchbay_channel_engine::session::ChatSession::ensure_session(
            self,
            &patchbay_channel_engine::session::EnsureSessionInput {
                workspace_id,
                agent_id,
                installation_id,
                sender,
                binding_key,
                binding_config,
                chat_type,
            },
        )
        .await
    }
    async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()> {
        patchbay_channel_engine::session::ChatSession::mark_pending_fresh(self, session_id).await
    }
    async fn append_user_message(
        &self,
        input: &patchbay_channel_engine::session::AppendInput,
    ) -> anyhow::Result<patchbay_channel_engine::resolvers::AppendResult> {
        patchbay_channel_engine::session::ChatSession::append_user_message(self, input).await
    }
    async fn bind_media_refs(
        &self,
        input: &patchbay_channel_engine::session::BindMediaInput,
    ) -> anyhow::Result<()> {
        patchbay_channel_engine::session::ChatSession::bind_media_refs(self, input).await
    }
}

// ---- installation routing ----

struct FeishuInstallationResolver {
    store: Arc<ChannelStore>,
}

#[async_trait]
impl InstallationResolver for FeishuInstallationResolver {
    async fn resolve_installation(
        &self,
        msg: &ChannelMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        let lm = lark_msg_from_raw(msg)?;
        let inst = match self.store.get_lark_installation_by_app_id(&lm.app_id).await {
            Ok(inst) => inst,
            Err(err) => {
                if crate::channel_store::is_no_rows(&err) {
                    return Err(ResolverError::InstallationNotFound.into());
                }
                return Err(err);
            }
        };
        Ok(ResolvedInstallation {
            id: inst.id,
            workspace_id: inst.workspace_id,
            agent_id: inst.agent_id,
            installer_user_id: inst.installer_user_id,
            active: crate::types::InstallationStatus(inst.status.clone()).is_active(),
            platform: Arc::new(inst),
            route_revision: 0,
        })
    }
}

// ---- identity ----

struct FeishuIdentityResolver {
    store: Arc<ChannelStore>,
}

#[async_trait]
impl IdentityResolver for FeishuIdentityResolver {
    async fn resolve_sender(
        &self,
        inst: &ResolvedInstallation,
        msg: &ChannelMessage,
    ) -> anyhow::Result<ResolvedIdentity> {
        let binding = match self
            .store
            .get_lark_user_binding_by_open_id(crate::params::GetUserBindingByOpenIdParams {
                installation_id: inst.id,
                channel_user_id: msg.source.sender_id.clone(),
            })
            .await
        {
            Ok(b) => b,
            Err(err) => {
                if crate::channel_store::is_no_rows(&err) {
                    return Err(ResolverError::SenderUnbound.into());
                }
                return Err(err);
            }
        };
        let is_member = self
            .store
            .is_workspace_member(inst.workspace_id, binding.patchbay_user_id)
            .await?;
        if !is_member {
            return Err(ResolverError::SenderNotMember.into());
        }
        Ok(ResolvedIdentity {
            user_id: binding.patchbay_user_id,
        })
    }
}

// ---- dedup ----

struct FeishuDeduper(Arc<ChannelStore>);

#[async_trait]
impl Deduper for FeishuDeduper {
    async fn claim(&self, installation_id: Uuid, message_id: &str) -> anyhow::Result<Uuid> {
        match self
            .0
            .claim_lark_inbound_dedup(crate::params::ClaimInboundDedupParams {
                installation_id,
                message_id: message_id.to_string(),
            })
            .await
        {
            Ok(claim) => Ok(claim.claim_token),
            Err(err) => {
                if crate::channel_store::is_no_rows(&err) {
                    Err(ResolverError::Duplicate.into())
                } else {
                    Err(err)
                }
            }
        }
    }

    async fn mark(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid) {
        let _ = self
            .0
            .mark_lark_inbound_dedup_processed(crate::params::MarkInboundDedupProcessedParams {
                installation_id,
                message_id: message_id.to_string(),
                claim_token,
            })
            .await;
    }

    async fn release(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid) {
        let _ = self
            .0
            .release_lark_inbound_dedup(crate::params::ReleaseInboundDedupParams {
                installation_id,
                message_id: message_id.to_string(),
                claim_token,
            })
            .await;
    }
}

// ---- session bind / append ----

struct FeishuSessionBinder {
    session: Arc<dyn FeishuChatSession>,
}

#[async_trait]
impl SessionBinder for FeishuSessionBinder {
    async fn ensure_session(&self, p: EnsureSessionParams) -> anyhow::Result<Uuid> {
        let (binding_key, config) = lark_session_routing(&p.message);
        self.session
            .ensure_session(
                p.installation.workspace_id,
                p.installation.agent_id,
                p.installation.id,
                p.sender,
                binding_key,
                config,
                p.message.source.chat_type.clone(),
            )
            .await
    }

    async fn mark_pending_fresh(&self, session_id: Uuid) -> anyhow::Result<()> {
        self.session.mark_pending_fresh(session_id).await
    }

    async fn append_message(
        &self,
        p: AppendParams,
    ) -> anyhow::Result<patchbay_channel_engine::resolvers::AppendResult> {
        let command_text = if p.message.command_text.is_empty() {
            p.message.text.clone()
        } else {
            p.message.command_text.clone()
        };
        self.session
            .append_user_message(&patchbay_channel_engine::session::AppendInput {
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
            })
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

struct FeishuAuditor {
    audit: Arc<dyn crate::chat::AuditLogger>,
}

#[async_trait]
impl Auditor for FeishuAuditor {
    async fn record_drop(&self, inst_id: Uuid, msg: &ChannelMessage, reason: &DropReason) {
        // event_type is platform-specific (read from Raw); a decode failure is
        // non-fatal — the drop is still worth auditing without it.
        let lm = lark_msg_from_raw(msg).unwrap_or_default();
        self.audit
            .record_drop(AuditDropParams {
                installation_id: inst_id,
                chat_id: ChatId(msg.source.chat_id.clone()),
                event_type: lm.event_type,
                lark_event_id: msg.event_id.clone(),
                lark_message_id: msg.message_id.clone(),
                reason: crate::types::DropReason(reason.0.clone()),
            })
            .await;
    }
}

// ---- outbound replier ----

/// The narrow Feishu-shaped replier seam the adapter wraps. Mirrors Go's
/// `OutcomeReplier` interface (Reply over Feishu-native types).
#[async_trait]
pub trait OutcomeReplier: Send + Sync {
    async fn reply(
        &self,
        ctx: CancellationToken,
        inst: &Installation,
        msg: &InboundMessage,
        res: &DispatchResultLike,
    );
}

/// The engine verdict mapped onto the Feishu DispatchResult shape the native
/// replier consumes (see [`crate::outcome_replier`]).
pub type DispatchResultLike = crate::feishu_types::DispatchResult;

/// Maps the engine verdict to the Feishu DispatchResult the OutcomeReplier
/// consumes. The Outcome/DropReason string values match 1:1.
pub fn dispatch_result_from_engine(res: &EngineResult) -> DispatchResultLike {
    DispatchResultLike {
        outcome: res.outcome.clone(),
        drop_reason: res
            .drop_reason
            .as_ref()
            .map(|d| crate::types::DropReason(d.0.clone())),
        installation_id: res.installation_id,
        chat_session_id: res.chat_session_id,
        sender_open_id: OpenId(res.sender.clone()),
        task_id: None,
        issue_id: res.issue_id,
        issue_number: res.issue_number,
        issue_identifier: res.issue_identifier.clone(),
        issue_title: res.issue_title.clone(),
        issue_duplicate: res.issue_duplicate,
        issue_usage_had_media: res.issue_usage_had_media,
    }
}

struct FeishuOutboundReplier {
    replier: Arc<dyn OutcomeReplier>,
}

#[async_trait]
impl OutboundReplier for FeishuOutboundReplier {
    async fn reply(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &ChannelMessage,
        res: &EngineResult,
    ) {
        let Some(lark_inst) = inst.platform.downcast_ref::<Installation>() else {
            return;
        };
        let lm = lark_msg_from_raw(msg).unwrap_or_default();
        self.replier
            .reply(ctx, lark_inst, &lm, &dispatch_result_from_engine(res))
            .await;
    }
}

// ---- typing indicator ----

struct FeishuTypingNotifier(Arc<TypingIndicatorManager>);

#[async_trait]
impl TypingNotifier for FeishuTypingNotifier {
    async fn on_ingested(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &ChannelMessage,
        session_id: Uuid,
    ) {
        let Some(lark_inst) = inst.platform.downcast_ref::<Installation>() else {
            return;
        };
        let lm = lark_msg_from_raw(msg).unwrap_or_default();
        self.0
            .add(ctx, lark_inst, session_id, &msg.message_id, &lm.create_time)
            .await;
    }

    /// Clears the reaction when the run trigger enqueued no task (agent
    /// offline / archived, or an enqueue failure) — the Patcher's bus-driven
    /// clear on chat-done / task-failed never fires for those, so without this
    /// the Typing reaction sticks.
    async fn on_settled(&self, ctx: CancellationToken, session_id: Uuid) {
        self.0.clear(ctx, session_id).await;
    }
}

/// Builds per-installation transport credentials from an installation row via
/// the shared credentials resolver (re-exported for the resolver wiring).
pub fn creds_for(
    creds_resolver: &dyn CredentialsResolver,
    inst: &Installation,
) -> anyhow::Result<crate::client::InstallationCredentials> {
    installation_credentials_for(creds_resolver, inst)
}

/// The narrow DB surface the typing-indicator manager needs (Go:
/// TypingIndicatorQueries.GetLarkInstallation).
#[async_trait]
pub trait InstallationLookup: Send + Sync {
    async fn get_lark_installation(&self, id: Uuid) -> anyhow::Result<Installation>;
}

#[async_trait]
impl InstallationLookup for ChannelStore {
    async fn get_lark_installation(&self, id: Uuid) -> anyhow::Result<Installation> {
        ChannelStore::get_lark_installation(self, id).await
    }
}

/// Owns the "processing" reaction lifecycle — re-exported here so the resolver
/// wiring has one import point. See [`crate::typing_indicator`] for the port.
pub use crate::typing_indicator::TypingIndicatorManager;

/// Re-export of the shared ApiClient seam the enricher/media paths consume.
pub type SharedApiClient = Arc<dyn ApiClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatType;

    fn channel(raw: serde_json::Value, source: patchbay_channel::Source) -> ChannelMessage {
        ChannelMessage {
            raw,
            source,
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_through_raw() {
        let lm = InboundMessage {
            app_id: "cli_a1".to_string(),
            event_type: "im.message.receive_v1".to_string(),
            create_time: "1700000000000".to_string(),
            ..Default::default()
        };
        let msg = channel(
            serde_json::to_value(&lm).unwrap(),
            patchbay_channel::Source::default(),
        );
        let back = lark_msg_from_raw(&msg).unwrap();
        assert_eq!(back.app_id, "cli_a1");
        assert_eq!(back.event_type, "im.message.receive_v1");
        assert_eq!(back.create_time, "1700000000000");
    }

    #[test]
    fn empty_and_garbage_raw_are_errors() {
        let msg = channel(serde_json::Value::Null, patchbay_channel::Source::default());
        assert!(lark_msg_from_raw(&msg).is_err());
        let junk = channel(serde_json::json!(3), patchbay_channel::Source::default());
        assert!(lark_msg_from_raw(&junk).is_err());
    }

    #[test]
    fn plain_chats_key_on_chat_id_without_config() {
        // The routing reads the CHANNEL envelope's source (Go: msg.Source).
        let mk = |chat_type: ChatType| {
            channel(
                serde_json::json!({}),
                patchbay_channel::Source {
                    chat_id: "oc_1".to_string(),
                    chat_type,
                    ..Default::default()
                },
            )
        };
        let (key, cfg) = lark_session_routing(&mk(ChatType::p2p()));
        assert_eq!(key, "oc_1");
        assert!(cfg.is_none());

        // Group without a topic behaves the same.
        let (key, cfg) = lark_session_routing(&mk(ChatType::group()));
        assert_eq!(key, "oc_1");
        assert!(cfg.is_none());
    }

    #[test]
    fn topics_compose_the_binding_key_and_config() {
        let msg = channel(
            serde_json::json!({}),
            patchbay_channel::Source {
                chat_id: "oc_9".to_string(),
                chat_type: ChatType::group(),
                thread_id: "om_t1".to_string(),
                ..Default::default()
            },
        );
        let (key, cfg) = lark_session_routing(&msg);
        assert_eq!(key, "oc_9:om_t1");
        let parsed: LarkBindingConfig =
            serde_json::from_value(cfg.expect("config present")).unwrap();
        assert_eq!(parsed.chat_id, "oc_9");
    }

    #[test]
    fn bot_name_preset_is_not_here_but_dispatch_mapping_keeps_fields() {
        // Guard against accidental field drift in dispatch_result_from_engine.
        let res = EngineResult {
            outcome: Some(patchbay_channel_engine::resolvers::Outcome::needs_binding()),
            drop_reason: None,
            installation_id: Some(Uuid::nil()),
            chat_session_id: None,
            sender: "ou_x".to_string(),
            issue_id: Some(Uuid::nil()),
            issue_number: 7,
            issue_identifier: "PB-7".to_string(),
            issue_title: "t".to_string(),
            issue_duplicate: true,
            issue_usage_had_media: false,
            run_scheduled: false,
        };
        let d = dispatch_result_from_engine(&res);
        assert!(d.outcome_is("needs_binding"));
        assert_eq!(d.sender_open_id.0, "ou_x");
        assert_eq!(d.issue_number, 7);
        assert_eq!(d.issue_identifier, "PB-7");
        assert!(d.issue_duplicate);
    }
}
