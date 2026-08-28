use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueMetadataArgs {
    #[command(subcommand)]
    pub(super) command: IssueMetadataCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssueMetadataCommand {
    #[command(about = "List all metadata keys on an issue")]
    List(IssueMetadataListArgs),
    #[command(about = "Get a single metadata key value")]
    Get(IssueMetadataKeyArgs),
    #[command(about = "Set a single metadata key value")]
    Set(IssueMetadataSetArgs),
    #[command(about = "Delete a single metadata key")]
    Delete(IssueMetadataDeleteArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssueMetadataListArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueMetadataKeyArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, help = "Metadata key (required)")]
    pub(super) key: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueMetadataDeleteArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, help = "Metadata key (required)")]
    pub(super) key: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueMetadataSetArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, help = "Metadata key (required)")]
    pub(super) key: Option<String>,
    #[arg(long, help = "Metadata value (required)")]
    pub(super) value: Option<String>,
    #[arg(long = "type", help = "Force value type: string, number, or bool")]
    pub(super) value_type: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
