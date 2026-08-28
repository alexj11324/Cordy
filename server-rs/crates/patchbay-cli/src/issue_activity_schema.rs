use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct IssueCommentArgs {
    #[command(subcommand)]
    pub(super) command: IssueCommentCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssueCommentCommand {
    #[command(about = "List comments on an issue")]
    List(IssueCommentListArgs),
    #[command(about = "Add a comment to an issue")]
    Add(IssueCommentAddArgs),
    #[command(about = "Delete a comment")]
    Delete {
        #[arg(value_name = "COMMENT-ID")]
        comment_id: String,
    },
    #[command(about = "Resolve a comment thread")]
    Resolve(IssueCommentResolutionArgs),
    #[command(about = "Unresolve a comment thread")]
    Unresolve(IssueCommentResolutionArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssueCommentListArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Only comments created after this RFC3339 timestamp")]
    pub(super) since: Option<String>,
    #[arg(long, help = "Return the thread containing this comment UUID")]
    pub(super) thread: Option<String>,
    #[arg(long, help = "Cap replies to the N most recent within --thread")]
    pub(super) tail: Option<i64>,
    #[arg(long, help = "Return the N most recently active threads")]
    pub(super) recent: Option<i64>,
    #[arg(long, help = "Only return top-level comments")]
    pub(super) roots_only: bool,
    #[arg(long, help = "Drop redundant fields from JSON output")]
    pub(super) compact: bool,
    #[arg(long, help = "Clip comment content to a short preview")]
    pub(super) summary: bool,
    #[arg(long, help = "Return resolved threads without folding")]
    pub(super) full: bool,
    #[arg(long, help = "Composite pagination timestamp cursor")]
    pub(super) before: Option<String>,
    #[arg(long, help = "Composite pagination UUID cursor")]
    pub(super) before_id: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct IssueCommentAddArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(
        long,
        help = "Comment content (decodes \\n, \\r, \\t, \\\\; use stdin to preserve literal backslashes)"
    )]
    pub(super) content: Option<String>,
    #[arg(long, help = "Read comment content from stdin")]
    pub(super) content_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read comment content from a UTF-8 file"
    )]
    pub(super) content_file: Option<String>,
    #[arg(
        long,
        help = "Allow content/attachment files outside the current workdir"
    )]
    pub(super) allow_external_file: bool,
    #[arg(long, help = "Parent comment ID to reply under")]
    pub(super) parent: Option<String>,
    #[arg(long, value_delimiter = ',', help = "File path(s) to attach")]
    pub(super) attachment: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueCommentResolutionArgs {
    #[arg(value_name = "COMMENT-ID")]
    pub(super) comment_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueRunsArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Show full task UUIDs in table output")]
    pub(super) full_id: bool,
}

#[derive(Debug, Args)]
pub(super) struct IssueRunMessagesArgs {
    #[arg(value_name = "TASK-ID")]
    pub(super) task_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Only return messages after this sequence number")]
    pub(super) since: i64,
    #[arg(long, help = "Issue ID/key to scope short task ID prefix resolution")]
    pub(super) issue: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct IssueMessageMainArgs {
    #[arg(value_name = "MAIN-TASK-ID")]
    pub(super) task_id: String,
    #[arg(
        long,
        help = "Confirmed next-step instruction for this Side Chat's main task"
    )]
    pub(super) content: Option<String>,
    #[arg(long, help = "Read the confirmed instruction from stdin")]
    pub(super) content_stdin: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueUsageArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueRerunArgs {
    #[arg(value_name = "ID")]
    pub(super) issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueCancelTaskArgs {
    #[arg(value_name = "TASK-ID")]
    pub(super) task_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Issue ID/key to scope short task ID prefix resolution")]
    pub(super) issue: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct IssueSearchArgs {
    #[arg(value_name = "QUERY")]
    pub(super) query: String,
    #[arg(long, default_value_t = 20, help = "Maximum number of results")]
    pub(super) limit: i64,
    #[arg(long, help = "Include done and cancelled issues")]
    pub(super) include_closed: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
