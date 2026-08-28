use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueLabelArgs {
    #[command(subcommand)]
    pub(super) command: IssueLabelCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssueLabelCommand {
    #[command(about = "List labels on an issue")]
    List(IssueLabelListArgs),
    #[command(about = "Attach a label to an issue")]
    Add(IssueLabelMutationArgs),
    #[command(about = "Remove a label from an issue")]
    Remove(IssueLabelMutationArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssueLabelListArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Show full UUIDs in table output")]
    pub(super) full_id: bool,
}

#[derive(Debug, Args)]
pub(super) struct IssueLabelMutationArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(value_name = "LABEL-ID")]
    pub(super) label_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Show full UUIDs in table output")]
    pub(super) full_id: bool,
}
