use clap::{Args, Subcommand};

use super::api::HealthProbeError;

#[derive(Debug, Args)]
pub(super) struct SetupArgs {
    #[arg(
        long,
        help = "Host/IP the browser callback URL points at when it can reach this CLI directly"
    )]
    pub(super) callback_host: Option<String>,
    #[command(subcommand)]
    pub(super) command: Option<SetupCommand>,
}

#[derive(Debug, Subcommand)]
pub(super) enum SetupCommand {
    #[command(about = "Configure Cordy Cloud")]
    Cloud(SetupCloudArgs),
    #[command(about = "Configure a self-hosted Cordy server")]
    SelfHost(SetupSelfHostArgs),
}

#[derive(Debug, Args)]
pub(super) struct SetupCloudArgs {
    #[arg(
        long,
        help = "Host/IP the browser callback URL points at when it can reach this CLI directly"
    )]
    pub(super) callback_host: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct SetupSelfHostArgs {
    #[arg(long, help = "Frontend URL used by the login flow")]
    pub(super) app_url: Option<String>,
    #[arg(
        long,
        default_value_t = 8080,
        help = "Backend port for local self-hosting"
    )]
    pub(super) port: u16,
    #[arg(
        long,
        default_value_t = 3000,
        help = "Frontend port for local self-hosting"
    )]
    pub(super) frontend_port: u16,
    #[arg(
        long,
        help = "Host/IP the browser callback URL points at when it can reach this CLI directly"
    )]
    pub(super) callback_host: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SetupError {
    #[error("setup health preflight failed: {0}")]
    HealthProbe(#[source] HealthProbeError),
    #[error("setup self-host requires --app-url when --server-url points at a remote host")]
    RemoteAppUrlRequired,
}
