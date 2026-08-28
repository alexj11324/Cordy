use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueSubscriberArgs {
    #[command(subcommand)]
    pub(super) command: IssueSubscriberCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssueSubscriberCommand {
    #[command(about = "List subscribers of an issue")]
    List {
        #[arg(value_name = "ISSUE-ID")]
        issue_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Subscribe a user or agent to an issue (defaults to the caller)")]
    Add(IssueSubscriberMutationArgs),
    #[command(about = "Unsubscribe a user or agent from an issue (defaults to the caller)")]
    Remove(IssueSubscriberMutationArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssueSubscriberMutationArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(
        long,
        help = "Member or agent name (fuzzy match; defaults to the caller)"
    )]
    pub(super) user: Option<String>,
    #[arg(long, help = "Member or agent UUID (mutually exclusive with --user)")]
    pub(super) user_id: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
