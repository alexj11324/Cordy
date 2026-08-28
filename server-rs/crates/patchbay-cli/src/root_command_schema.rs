use clap::{Args, ValueEnum};
use std::time::Duration;

use super::parse_cli_duration;

#[derive(Debug, Args)]
pub(super) struct UpdateArgs {
    #[arg(
        long,
        value_parser = parse_cli_duration,
        help = "Maximum time to wait for the release archive download"
    )]
    pub(super) download_timeout: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(super) enum VersionOutput {
    #[default]
    Text,
    Json,
}
