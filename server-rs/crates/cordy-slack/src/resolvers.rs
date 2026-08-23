//! Slack resolvers connecting the channel-agnostic inbound pipeline to Slack
//! installation routing, identity binding, deduplication, session persistence,
//! auditing, and the typing indicator. Port of
//! `server/internal/integrations/slack/resolvers.go`. Shared channel state is
//! stored through the generic channel tables.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use cordy_channel::InboundMessage;
use cordy_channel_engine::resolvers::{
    AppendParams, AppendResult, BindMediaParams, DropReason, EnsureSessionParams, MediaResolver,
    OutboundReplier, ResolvedIdentity, ResolvedInstallation, ResolverError, ResolverSet,
    TypingNotifier,
};
use cordy_channel_engine::session::{ChatSession, SessionTitles};
use cordy_db::models::{ChannelInstallation, ChannelUserBinding};
use cordy_db::queries::channel::{
    claim_channel_inbound_dedup, find_reusable_channel_user_binding,
    get_channel_installation_by_app_id, get_channel_user_binding_by_user_id,
    mark_channel_inbound_dedup_processed, record_channel_inbound_drop,
    release_channel_inbound_dedup,
};
use cordy_db::queries::member::get_member_by_user_and_workspace;

use crate::config::DecrypterArc;
use crate::raw::decode_slack_raw;
use crate::TYPE_SLACK;

/// The issue.origin_type label for issues created via the Slack /issue command.
pub const ORIGIN_SLACK_CHAT: &str = "slack_chat";

/// Assembles the Slack implementation of each inbound pipeline stage. The
/// replier sends binding and status notices (wired by the caller into
/// [`ResolverSet::replier`]), typing manages the processing reaction, and media
/// stores inbound attachments. Each optional dependency may be None to disable
/// only that capability while preserving normal Slack message ingestion.
pub fn new_slack_resolver_set(
    pool: PgPool,
    decrypt: Option<DecrypterArc>,
    typing: Option<Arc<super::typing_indicator::TypingIndicatorManager>>,
    media: Option<Arc<dyn MediaResolver>>,
    replier: Option<Arc<dyn OutboundReplier>>,
) -> ResolverSet {
    let session = ChatSession::new(
        pool.clone(),
        cordy_channel::Type(TYPE_SLACK.to_string()),
        SessionTitles {
            group: "Slack channel".to_string(),
            direct: "Slack direct message".to_string(),
            fallback: "Slack chat".to_string(),
        },
    );
    ResolverSet {
        installation: Some(Arc::new(InstallationResolver { pool: pool.clone() })),
        identity: Some(Arc::new(IdentityResolver { pool: pool.clone() })),
        dedup: Some(Arc::new(Deduper { pool: pool.clone() })),
        session: Some(Arc::new(SessionBinder { session })),
        audit: Some(Arc::new(Auditor { pool })),
        media,
        replier,
        typing: typing
            .map(|mgr| Arc::new(SlackTypingNotifier { mgr, decrypt }) as Arc<dyn TypingNotifier>),
        validated: None,
        origin_type: ORIGIN_SLACK_CHAT.to_string(),
    }
}

/// The opaque outbound routing persisted on the chat binding's config. When
/// the binding key is a composite (Slack channel thread), the real channel id
/// lives here so the outbound path can post back.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SlackBindingConfig {
    #[serde(default, rename = "channel_id")]
    pub channel_id: String,
}

/// Derives, from one inbound Slack message, the three things the session layer
/// needs kept distinct:
///
///   - binding_key: the session-isolation key (stored as channel_chat_id). A
///     DM is one continuous session per channel, so the key is the channel id.
///     A channel/group message is isolated by THREAD ROOT — key =
///     "channel:root" — so two @bot threads in one channel are two sessions.
///     The thread root is the inbound thread_ts when replying in a thread,
///     else the message ts (a top-level @mention starts a new root).
///   - config: the real channel id, so outbound works even when the key is
///     composite.
///   - reply_thread: the thread_ts to reply into (the thread root for groups;
///     the inbound thread for DMs, which may be empty for a top-level send).
///
/// It is a pure function so the isolation contract is unit-tested without a DB.
pub fn slack_session_routing(msg: &InboundMessage) -> (String, serde_json::Value, String) {
    let chat_id = msg.source.chat_id.clone();
    let cfg = serde_json::to_value(SlackBindingConfig {
        channel_id: chat_id.clone(),
    })
    .unwrap_or(serde_json::json!({}));
    if msg.source.chat_type == cordy_channel::ChatType::p2p() {
        return (chat_id, cfg, msg.source.thread_id.clone());
    }
    // The thread root is the inbound thread_ts when the @mention is a reply
    // inside an existing thread, else the message's own ts (a top-level mention
    // becomes the root the bot threads its reply under). Either way the root is
    // recoverable later from the binding (channel_chat_id suffix /
    // last_thread_id), which is what the history reader uses to read the
    // thread.
    let mut thread_root = msg.source.thread_id.clone();
    if thread_root.is_empty() {
        thread_root = msg.message_id.clone();
    }
    (format!("{chat_id}:{thread_root}"), cfg, thread_root)
}

/// Reads the real Slack team id from a stored installation config, or "" if
/// absent/undecodable. Unlike decode_credentials / DecodePublicConfig it does
/// NOT fall back to app_id: team routing and identity reuse must match the
/// actual Slack workspace, and app_id != team_id for BYO apps.
pub fn install_team_id(install_config_json: &serde_json::Value) -> String {
    #[derive(Default, serde::Deserialize)]
    struct Cfg {
        #[serde(default, rename = "team_id")]
        team_id: String,
    }
    serde_json::from_value::<Cfg>(install_config_json.clone())
        .map(|c| c.team_id)
        .unwrap_or_default()
}

/// Reports whether an installation (its stored config) may serve events from
/// event_team_id. Inbound routing keys on api_app_id, which identifies the
/// Slack APP, not the Slack workspace: a BYO app distributed / installed into
/// another Slack workspace emits events carrying the SAME app id. So we
/// additionally require the event's team to match the team the installed bot
/// belongs to. An installation with no recorded team (legacy) is permissive.
pub fn installation_serves_team(
    install_config_json: &serde_json::Value,
    event_team_id: &str,
) -> bool {
    let team_id = install_team_id(install_config_json);
    team_id.is_empty() || team_id == event_team_id
}

// ---- installation routing ----

pub struct InstallationResolver {
    pool: PgPool,
}

#[async_trait]
impl cordy_channel_engine::resolvers::InstallationResolver for InstallationResolver {
    async fn resolve_installation(
        &self,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedInstallation> {
        let raw = decode_slack_raw(msg)?;
        // Route by the event's api_app_id: each BYO installation stores its
        // real Slack app id in the routing-key slot (config->>'app_id'), and
        // the per-installation Socket Mode connection only ever delivers
        // events for its own app, so api_app_id uniquely identifies the
        // installation.
        let inst = get_channel_installation_by_app_id(&self.pool, TYPE_SLACK, &raw.api_app_id)
            .await?
            .ok_or(ResolverError::InstallationNotFound)?;
        if !installation_serves_team(&inst.config, &raw.team_id) {
            return Err(ResolverError::InstallationNotFound.into());
        }
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

// ---- identity ----

pub struct IdentityResolver {
    pool: PgPool,
}

#[async_trait]
impl cordy_channel_engine::resolvers::IdentityResolver for IdentityResolver {
    async fn resolve_sender(
        &self,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
    ) -> anyhow::Result<ResolvedIdentity> {
        let sender_id = msg.source.sender_id.as_str();
        let found = get_channel_user_binding_by_user_id(&self.pool, inst.id, sender_id).await?;
        let (binding, reused) = match found {
            Some(b) => (b, false),
            None => {
                // Not linked to THIS installation. Before prompting, reuse a
                // link the same Slack user already made to another installation
                // of the same team in this workspace (MUL-3911): one link per
                // Slack workspace, not per app.
                match self.reusable_binding(inst, sender_id).await? {
                    Some(cand) => (cand, true),
                    None => return Err(ResolverError::SenderUnbound.into()),
                }
            }
        };
        // Binding existence no longer proves membership (no FK); re-check. For
        // a reused link this also gates materialization: we never persist a
        // binding for a user who has since left the workspace.
        match get_member_by_user_and_workspace(&self.pool, binding.cordy_user_id, inst.workspace_id)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                if reused {
                    // Same human, no longer a member: prompt a fresh link rather
                    // than surface "not a member" for an app they never linked.
                    return Err(ResolverError::SenderUnbound.into());
                }
                return Err(ResolverError::SenderNotMember.into());
            }
            Err(e) => return Err(anyhow::anyhow!("{e:#}")),
        }
        if reused {
            // Materialize the reused link as a binding on THIS installation so
            // later messages resolve on the fast per-installation path and are
            // pruned with the member like any other. Idempotent via ON
            // CONFLICT; a concurrent first message that already wrote it
            // returns the same row.
            use cordy_db::queries::channel::create_channel_user_binding;
            create_channel_user_binding(
                &self.pool,
                inst.workspace_id,
                binding.cordy_user_id,
                inst.id,
                TYPE_SLACK,
                sender_id,
                &serde_json::json!({}),
            )
            .await
            .map_err(|e| anyhow::anyhow!("materialize reused slack binding: {e:#}"))?;
        }
        Ok(ResolvedIdentity {
            user_id: binding.cordy_user_id,
        })
    }
}

impl IdentityResolver {
    /// Looks for a link the same Slack user already made to ANOTHER
    /// installation of the SAME workspace + SAME Slack team, so a second app in
    /// one Slack workspace need not re-prompt (MUL-3911). Ok(None) means "no
    /// reuse — prompt to link": the installation records no team (legacy), its
    /// Platform is not a ChannelInstallation, or no matching binding exists.
    async fn reusable_binding(
        &self,
        inst: &ResolvedInstallation,
        sender_id: &str,
    ) -> anyhow::Result<Option<ChannelUserBinding>> {
        let Some(ci) = inst.platform.downcast_ref::<ChannelInstallation>() else {
            return Ok(None);
        };
        let team_id = install_team_id(&ci.config);
        if team_id.is_empty() {
            return Ok(None);
        }
        find_reusable_channel_user_binding(
            &self.pool,
            inst.workspace_id,
            TYPE_SLACK,
            sender_id,
            &team_id,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))
    }
}

// ---- dedup ----

struct Deduper {
    pool: PgPool,
}

#[async_trait]
impl cordy_channel_engine::resolvers::Deduper for Deduper {
    async fn claim(&self, installation_id: Uuid, message_id: &str) -> anyhow::Result<Uuid> {
        match claim_channel_inbound_dedup(&self.pool, installation_id, message_id).await? {
            Some(row) => Ok(row.claim_token),
            // A duplicate INSERT under the unique key surfaces as ErrNoRows in
            // Go; the Rust generator folds both into None.
            None => Err(ResolverError::Duplicate.into()),
        }
    }

    async fn mark(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid) {
        if let Err(e) = mark_channel_inbound_dedup_processed(
            &self.pool,
            installation_id,
            message_id,
            claim_token,
        )
        .await
        {
            tracing::warn!(installation = %installation_id, error = %e, "slack dedup mark failed");
        }
    }

    async fn release(&self, installation_id: Uuid, message_id: &str, claim_token: Uuid) {
        if let Err(e) =
            release_channel_inbound_dedup(&self.pool, installation_id, message_id, claim_token)
                .await
        {
            tracing::warn!(installation = %installation_id, error = %e, "slack dedup release failed");
        }
    }
}

// ---- session bind / append ----

struct SessionBinder {
    session: ChatSession,
}

#[async_trait]
impl cordy_channel_engine::resolvers::SessionBinder for SessionBinder {
    async fn ensure_session(&self, p: EnsureSessionParams) -> anyhow::Result<Uuid> {
        let (binding_key, config, _) = slack_session_routing(&p.message);
        self.session
            .ensure_session(&cordy_channel_engine::session::EnsureSessionInput {
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
        let (_, _, reply_thread) = slack_session_routing(&p.message);
        let mut command_text = p.message.command_text.clone();
        if command_text.is_empty() {
            command_text = p.message.text.clone();
        }
        self.session
            .append_user_message(&cordy_channel_engine::session::AppendInput {
                session_id: p.session_id,
                sender: p.sender,
                installation_id: p.installation_id,
                body: p.message.text.clone(),
                command_text,
                message_id: p.message.message_id.clone(),
                thread_id: reply_thread,
                claim_token: p.claim_token,
                media_pending_seconds: p.media_pending_seconds,
                force_fresh: p.message.force_fresh,
            })
            .await
    }

    async fn bind_media(&self, p: BindMediaParams) -> anyhow::Result<()> {
        self.session
            .bind_media_refs(&cordy_channel_engine::session::BindMediaInput {
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

struct Auditor {
    pool: PgPool,
}

#[async_trait]
impl cordy_channel_engine::resolvers::Auditor for Auditor {
    async fn record_drop(&self, inst_id: Uuid, msg: &InboundMessage, reason: &DropReason) {
        // event_type is best-effort; a decode miss still audits the drop.
        let event_type = decode_slack_raw(msg)
            .map(|raw| raw.event_type)
            .unwrap_or_default();
        let res = record_channel_inbound_drop(
            &self.pool,
            TYPE_SLACK,
            &event_type,
            &reason.0,
            inst_id,
            opt_str(&msg.source.chat_id),
            opt_str(&msg.event_id),
            opt_str(&msg.message_id),
            cordy_db::dbid::new_v7(),
        )
        .await;
        if let Err(e) = res {
            tracing::warn!(error = %e, "slack drop audit failed");
        }
    }
}

fn opt_str(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ---- typing indicator ----

/// Bridges the engine TypingNotifier seam onto the manager. OnIngested fires
/// when a Slack message is successfully ingested; it reacts to the user's
/// message (channel = Source.ChatID, ts = MessageID) so the user sees the bot
/// is processing it. The resolved installation carries the encrypted config in
/// its platform row — the InstallationResolver stashed the ChannelInstallation
/// there, the documented adapter boundary the core never reads.
pub struct SlackTypingNotifier {
    mgr: Arc<super::typing_indicator::TypingIndicatorManager>,
    decrypt: Option<DecrypterArc>,
}

#[async_trait]
impl TypingNotifier for SlackTypingNotifier {
    async fn on_ingested(
        &self,
        ctx: tokio_util::sync::CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        session_id: Uuid,
    ) {
        let Some(ci) = inst.platform.downcast_ref::<ChannelInstallation>() else {
            return;
        };
        self.mgr
            .add(
                ctx,
                ci,
                self.decrypt.as_ref(),
                session_id,
                &msg.source.chat_id,
                &msg.message_id,
            )
            .await;
    }

    /// Clears the reaction when the run trigger enqueued no task (agent
    /// offline / archived, or an enqueue failure) — the bus-driven clear on
    /// chat-done / task-failed never fires for those, so without this the 👀
    /// sticks.
    async fn on_settled(&self, ctx: tokio_util::sync::CancellationToken, session_id: Uuid) {
        self.mgr.clear(ctx, session_id, self.decrypt.as_ref()).await;
    }
}

// Re-exported so wiring code can name the decrypter without importing config.
pub use crate::config::decrypt_token as _decrypt_token_reexport;
