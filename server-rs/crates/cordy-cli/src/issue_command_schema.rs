use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueArgs {
    #[command(subcommand)]
    pub(super) command: IssueCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssueCommand {
    #[command(about = "List issues in the workspace")]
    List(IssueListArgs),
    #[command(about = "Get issue details")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(
        name = "pull-requests",
        alias = "prs",
        about = "List pull requests linked to an issue"
    )]
    PullRequests {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Manage pull requests linked to an issue")]
    PullRequest(IssuePullRequestArgs),
    #[command(
        alias = "subissues",
        about = "List an issue's sub-issues grouped by stage"
    )]
    Children {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
    },
    #[command(about = "Create a new issue")]
    Create(IssueCreateArgs),
    #[command(about = "Update an issue")]
    Update(IssueUpdateArgs),
    #[command(about = "Assign an issue to a member, agent, or squad")]
    Assign(IssueAssignArgs),
    #[command(about = "Change issue status")]
    Status(IssueStatusArgs),
    #[command(about = "Move an issue within its status column")]
    Reorder(IssueReorderArgs),
    #[command(about = "Work with issue comments")]
    Comment(IssueCommentArgs),
    #[command(about = "List execution history for an issue")]
    Runs(IssueRunsArgs),
    #[command(name = "run-messages", about = "List messages for an execution")]
    RunMessages(IssueRunMessagesArgs),
    #[command(about = "Show aggregated token usage for an issue")]
    Usage(IssueUsageArgs),
    #[command(about = "Re-enqueue an issue assignment as a fresh task")]
    Rerun(IssueRerunArgs),
    #[command(
        name = "cancel-task",
        about = "Cancel a running or queued task (interrupts in-flight agent)"
    )]
    CancelTask(IssueCancelTaskArgs),
    #[command(about = "Search issues by title, description, or comments")]
    Search(IssueSearchArgs),
    #[command(about = "Work with issue subscribers")]
    Subscriber(IssueSubscriberArgs),
    #[command(about = "Manage labels on an issue")]
    Label(IssueLabelArgs),
    #[command(about = "Manage per-issue metadata (KV)")]
    Metadata(IssueMetadataArgs),
    #[command(
        alias = "history",
        about = "Chronological issue history — status, assignee, and comments"
    )]
    Timeline(IssueTimelineArgs),
    #[command(about = "Manage custom property values on an issue")]
    Property(IssuePropertyArgs),
}
