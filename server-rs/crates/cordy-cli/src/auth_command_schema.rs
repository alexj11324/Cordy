use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct AuthArgs {
    #[command(subcommand)]
    pub(super) command: AuthCommand,
}

#[derive(Debug, Args)]
pub(super) struct LoginArgs {
    #[arg(long, help = "Authenticate using a personal access token")]
    pub(super) token: Option<String>,
    #[arg(
        long,
        help = "Host/IP the browser callback URL points at when it can reach this CLI directly"
    )]
    pub(super) callback_host: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(super) enum AuthCommand {
    #[command(about = "Show current authentication status")]
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Remove stored authentication token")]
    Logout,
}
