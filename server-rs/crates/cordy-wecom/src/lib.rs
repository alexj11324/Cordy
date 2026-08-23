//! WeCom (企业微信) smart-bot ("智能机器人" / aibot) adapter — port of
//! `server/internal/integrations/wecom`.
//!
//! Unlike the internal customer-service ("内部客服号") flow which is
//! HTTP-callback based, the smart-bot flow is a client-initiated WebSocket
//! long connection against `wss://openws.work.weixin.qq.com`. Each
//! installation carries a `(bot_id, secret)` pair; after the WS handshake the
//! client subscribes with `aibot_subscribe` and thereafter receives
//! `aibot_msg_callback` events and sends `aibot_send_msg` /
//! `aibot_respond_msg` / `aibot_upload_media_*` over the same socket. No
//! public callback URL is required.
//!
//! One installation = one bot = one WebSocket. WeCom allows only one active
//! connection per bot; a second connection kicks the first with a
//! `disconnected_event`. That single-active-connection guarantee lines up
//! with the engine Supervisor's WS lease, so the multi-replica invariant
//! (at most one active connection per installation across processes) already
//! holds without wecom-specific code.
//!
//! Module map (Go file → Rust module):
//!
//! | Go                    | Rust                       |
//! |-----------------------|----------------------------|
//! | types.go              | [`types`]                  |
//! | ws_frame.go           | [`ws_frame`]               |
//! | trace.go              | [`trace`]                  |
//! | metrics.go            | [`metrics`]                |
//! | ws_sender.go          | [`ws_sender`]              |
//! | senders_registry.go   | [`senders_registry`]       |
//! | wecom_channel.go      | [`wecom_channel`]          |
//! | credential_probe.go   | [`credential_probe`]       |
//! | credentials.go        | [`credentials`]            |
//! | store.go              | [`store`]                  |
//! | installation.go       | [`installation`]           |
//! | markdown.go           | [`markdown`]               |
//! | media_crypt.go        | [`media_crypt`]            |
//! | media_stream.go       | [`media_stream`]           |
//! | media_guard.go        | [`media_guard`]            |
//! | media_download.go     | [`media_download`]         |
//! | media_ingest.go       | [`media_ingest`]           |
//! | media_upload.go       | [`media_upload`]           |
//! | outbound_media.go     | [`outbound_media`]         |
//!
//! Inbound handles text, the transcript WeCom returns for a voice note,
//! photos, files, videos and 图文混排 ([`media_ingest`] downloads and
//! decrypts what a callback points at); a kind it cannot read still gets a
//! short receipt.
//!
//! Outbound file delivery cannot report back to the agent that produced the
//! file: the send into the room runs after the run has ended, so a delivery
//! that is shed, refused by WeCom, or lost with the socket is told to the
//! person in the chat ([`outbound_media`]) and never to the agent.

pub mod credential_probe;
pub mod credentials;
pub mod inbox_message;
pub mod installation;
pub mod markdown;
pub mod media_crypt;
pub mod media_download;
pub mod media_guard;
pub mod media_ingest;
pub mod media_stream;
pub mod media_upload;
pub mod metrics;
pub mod outbound_media;
pub mod replier;
pub mod resolvers;
pub mod senders_registry;
pub mod store;
pub mod trace;
pub mod types;
pub mod wecom_channel;
pub mod ws_frame;
pub mod ws_sender;

/// The channel discriminator for the WeCom smart-bot adapter. It is defined
/// here alongside the wecom-specific types (rather than in `cordy-channel`)
/// so a build that excludes wecom does not force a channel-core edit; slack
/// follows the same pattern.
///
/// Port note: Go's `TypeWecom channel.Type`; Rust keeps the string constant
/// plus a constructor because [`cordy_channel::Type`] is an owned newtype.
pub const TYPE_WECOM: &str = "wecom";

/// The string form persisted in `channel_installation.channel_type`.
pub const CHANNEL_TYPE_WECOM: &str = TYPE_WECOM;

/// Builds the [`cordy_channel::Type`] this adapter registers under.
pub fn type_wecom() -> cordy_channel::Type {
    cordy_channel::Type(TYPE_WECOM.to_string())
}
