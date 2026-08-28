use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct ConfigArgs {
    #[command(subcommand)]
    pub(super) command: Option<ConfigCommand>,
}

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    #[command(about = "Show current CLI configuration")]
    Show {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Set a CLI configuration value")]
    Set { key: String, value: String },
}
