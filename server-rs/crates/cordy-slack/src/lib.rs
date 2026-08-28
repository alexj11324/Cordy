//! Slack adapter.
//!
//! Slack uses the bring-your-own-app (BYO) model (PB-3666): each agent's
//! Slack app is created and installed by the workspace admin, who pastes its
//! bot token (`xoxb-`) and app-level token (`xapp-`) into Cordy. Each
//! `channel_installation` therefore carries its OWN app-level token and gets
//! its OWN Socket Mode connection, supervised per-installation by the engine —
//! so several agents can each have a distinct bot identity in one Slack
//! workspace. Installations are keyed and routed by the real Slack app id
//! (`config->>'app_id'` == the inbound event's `api_app_id`).
//!
//! Module map (Go file → Rust module):
//!
//! | Go                  | Rust                  |
//! |---------------------|-----------------------|
//! | config.go           | [`config`]            |
//! | binding.go          | [`binding`]           |
//! | inbound.go (subset) | [`raw`] + [`inbound`] |
//! | slack-go SDK        | [`client`]            |
//! | install.go          | [`install`]           |
//! | resolvers.go        | [`resolvers`]         |
//! | history.go          | [`history`]           |
//! | media_ingest.go     | [`media_ingest`]      |
//! | mrkdwn.go           | [`mrkdwn`]            |
//! | typing_indicator.go | [`typing_indicator`]  |
//! | slash_command.go    | [`slash_command`]     |
//! | outbound.go         | [`outbound`]          |
//! | replier.go          | [`replier`]           |
//! | channel.go + slack_channel.go | [`channel`] + [`socket_mode`] |

pub mod binding;
pub mod channel;
pub mod client;
pub mod config;
pub mod history;
pub mod inbound;
pub mod install;
pub mod media_ingest;
pub mod mrkdwn;
pub mod outbound;
pub mod raw;
pub mod replier;
pub mod resolvers;
pub mod slash_command;
pub mod socket_mode;
pub mod typing_indicator;

/// Channel discriminator for the Slack adapter. Defined here (not in the
/// channel core) so registering the platform never edits the core.
pub const TYPE_SLACK: &str = "slack";

/// The issue.origin_type label for issues created via the Slack /issue path.
pub const ORIGIN_SLACK_CHAT: &str = "slack_chat";
