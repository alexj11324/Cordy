use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssuePullRequestArgs {
    #[command(subcommand)]
    pub(super) command: IssuePullRequestCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssuePullRequestCommand {
    #[command(about = "Explicitly attach an existing GitHub pull request to an issue")]
    Attach(IssuePullRequestAttachArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssuePullRequestAttachArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(
        long,
        help = "GitHub pull request URL: https://github.com/{owner}/{repo}/pull/{number}"
    )]
    pub(super) url: String,
    #[arg(
        long,
        help = "Optional PR title, used only when the workspace has no GitHub App installed"
    )]
    pub(super) title: Option<String>,
    #[arg(long, help = "Optional PR state: open, closed, merged, or draft")]
    pub(super) state: Option<String>,
    #[arg(long, help = "Optional head branch name")]
    pub(super) branch: Option<String>,
    #[arg(long, help = "Optional head commit SHA")]
    pub(super) head_sha: Option<String>,
    #[arg(
        long,
        help = "Record explicit close intent so a merged PR may move the issue to Done"
    )]
    pub(super) close_intent: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
