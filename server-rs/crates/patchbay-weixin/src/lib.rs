//! Native adapter for Tencent's personal-WeChat iLink HTTP API.
//!
//! This is intentionally separate from `patchbay-wecom`: WeCom authenticates a
//! corporate smart bot over WebSocket, while iLink authorizes a personal
//! WeChat bot by QR code and receives messages by HTTP long polling.

pub mod api;
pub mod channel;
pub mod config;
pub mod inbound;
pub mod install;
pub mod outbound;
pub mod replier;
pub mod resolvers;

pub const TYPE_WEIXIN: &str = "weixin";
