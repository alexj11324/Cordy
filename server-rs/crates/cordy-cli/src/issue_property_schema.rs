use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssuePropertyArgs {
    #[command(subcommand)]
    pub(super) command: IssuePropertyCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssuePropertyCommand {
    #[command(about = "List custom property values set on an issue")]
    List(IssuePropertyListArgs),
    #[command(about = "Set a custom property value on an issue")]
    Set(IssuePropertyMutationArgs),
    #[command(about = "Remove a custom property value from an issue")]
    Unset(IssuePropertyUnsetArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssuePropertyListArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssuePropertyMutationArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, help = "Property name or UUID (required)")]
    pub(super) name: Option<String>,
    #[arg(
        long,
        help = "Property value (required; see --help for per-type forms)"
    )]
    pub(super) value: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssuePropertyUnsetArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, help = "Property name or UUID (required)")]
    pub(super) name: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
