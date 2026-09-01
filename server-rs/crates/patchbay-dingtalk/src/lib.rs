//! DingTalk adapter.
//!
//! Uses the bring-your-own-app (BYO) model: a workspace admin creates their
//! own DingTalk Stream-mode robot and pastes its AppKey (client id) and
//! AppSecret (client secret) into Patchbay. Each channel_installation carries its
//! OWN AppSecret and gets its OWN Stream-mode connection, supervised
//! per-installation by the engine like Feishu and Slack ([`dingtalk_channel`])
//! — so several agents can each have a distinct bot identity in one DingTalk
//! organization.
//!
//! Each installation's Stream connection only ever delivers events for its own
//! robot, so the per-installation connection stamps its AppKey into the inbound
//! envelope and the resolver routes on it (`config->>'app_id'`). Unlike Slack's
//! static bot token, DingTalk outbound needs a short-lived access_token minted
//! from AppKey/AppSecret, so the outbound path caches it like Feishu's
//! tenant_access_token ([`client`]).
//!
//! Maintenance: this package is COMMUNITY-MAINTAINED. Its maintainers, the
//! support boundary and the retirement rule are published at
//! <https://patchbay.aspectlylabs.com/docs/community-maintained>. That page is the single source
//! of truth — record ownership changes there, not here.

pub mod ack;
pub mod binding;
pub mod byo_install;
pub mod client;
pub mod config;
pub mod dingtalk_channel;
pub mod dispatch;
pub mod inbound;
pub mod install;
pub mod markdown;
pub mod media;
pub mod outbound;
pub mod outbound_send;
pub mod replier;
pub mod resolvers;
pub mod ws_connector;
pub mod ws_endpoint;
pub mod ws_frame;

/// Channel discriminator for the DingTalk adapter. Defined here (not in the
/// channel core) on purpose: registering a new platform must not require
/// editing the core, so the Type value lives with its adapter.
pub const TYPE_DINGTALK: &str = "dingtalk";

/// The [`patchbay_channel::Type`] value for this adapter (see [`TYPE_DINGTALK`]).
pub fn channel_type() -> patchbay_channel::Type {
    patchbay_channel::Type(TYPE_DINGTALK.to_string())
}

/// The issue.origin_type label for issues created through the DingTalk /issue
/// command. Keep it aligned with the database CHECK constraint, like the
/// existing lark_chat and slack_chat channel origins.
pub const ORIGIN_DINGTALK_CHAT: &str = "dingtalk_chat";

/// Recovers the adapter's own installation row from the opaque platform slot
/// the Router carries on [`patchbay_channel_engine::resolvers::ResolvedInstallation`].
/// Go: `inst.Platform.(db.ChannelInstallation)`.
pub(crate) fn db_row_from_platform(
    inst: &patchbay_channel_engine::resolvers::ResolvedInstallation,
) -> anyhow::Result<std::sync::Arc<patchbay_db::models::ChannelInstallation>> {
    inst.platform
        .clone()
        .downcast::<patchbay_db::models::ChannelInstallation>()
        .map_err(|_| anyhow::anyhow!("installation platform row unavailable"))
}
