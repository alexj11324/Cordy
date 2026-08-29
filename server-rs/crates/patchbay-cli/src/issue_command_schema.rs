use clap::{ArgGroup, Args, Subcommand};

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
    #[command(about = "Assign an issue to a member, agent, or team")]
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
    #[command(
        name = "message-main",
        about = "Send a confirmed Side Chat instruction to its Agent's main task"
    )]
    MessageMain(IssueMessageMainArgs),
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

#[derive(Debug, Args)]
pub(super) struct IssueCreateArgs {
    #[arg(long, help = "Issue title (required)")]
    pub(super) title: Option<String>,
    #[arg(
        long,
        help = "Issue description (decodes \\n, \\r, \\t, \\\\; pipe via --description-stdin to preserve literal backslashes)"
    )]
    pub(super) description: Option<String>,
    #[arg(
        long,
        help = "Read issue description from stdin (preserves multi-line content verbatim)"
    )]
    pub(super) description_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read issue description from a UTF-8 file"
    )]
    pub(super) description_file: Option<String>,
    #[arg(
        long,
        help = "Allow --description-file / --attachment outside the current working directory"
    )]
    pub(super) allow_external_file: bool,
    #[arg(long, help = "Issue status")]
    pub(super) status: Option<String>,
    #[arg(long, help = "Issue priority")]
    pub(super) priority: Option<String>,
    #[arg(long, help = "Assignee name (member, agent, or team; fuzzy match)")]
    pub(super) assignee: Option<String>,
    #[arg(
        long,
        help = "Assignee UUID — member, agent, or team (mutually exclusive with --assignee)"
    )]
    pub(super) assignee_id: Option<String>,
    #[arg(long, help = "Parent issue ID")]
    pub(super) parent: Option<String>,
    #[arg(
        long,
        help = "Stage ordinal (>=1) grouping this sub-issue into an ordered barrier group under its parent"
    )]
    pub(super) stage: Option<i64>,
    #[arg(long, help = "Project ID")]
    pub(super) project: Option<String>,
    #[arg(long, help = "Start date (calendar day, YYYY-MM-DD)")]
    pub(super) start_date: Option<String>,
    #[arg(long, help = "Due date (calendar day, YYYY-MM-DD)")]
    pub(super) due_date: Option<String>,
    #[arg(
        long,
        help = "Allow creating an issue even when an active duplicate exists"
    )]
    pub(super) allow_duplicate: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
    #[arg(long, value_delimiter = ',', help = "File path(s) to attach")]
    pub(super) attachment: Vec<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Existing attachment UUID(s) to bind"
    )]
    pub(super) attachment_id: Vec<String>,
}

#[derive(Debug, Args)]
pub(super) struct IssueUpdateArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "New title")]
    pub(super) title: Option<String>,
    #[arg(
        long,
        help = "New description (decodes \\n, \\r, \\t, \\\\; pipe via --description-stdin to preserve literal backslashes)"
    )]
    pub(super) description: Option<String>,
    #[arg(long, help = "Read new description from stdin")]
    pub(super) description_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read new description from a UTF-8 file"
    )]
    pub(super) description_file: Option<String>,
    #[arg(
        long,
        help = "Allow --description-file outside the current working directory"
    )]
    pub(super) allow_external_file: bool,
    #[arg(long, help = "New status")]
    pub(super) status: Option<String>,
    #[arg(long, help = "New priority")]
    pub(super) priority: Option<String>,
    #[arg(
        long,
        help = "New assignee name (member, agent, or team; fuzzy match)"
    )]
    pub(super) assignee: Option<String>,
    #[arg(long, help = "New assignee UUID — member, agent, or team")]
    pub(super) assignee_id: Option<String>,
    #[arg(long, help = "Project ID; pass an empty string to clear")]
    pub(super) project: Option<String>,
    #[arg(long, help = "New start date; pass an empty string to clear")]
    pub(super) start_date: Option<String>,
    #[arg(long, help = "New due date; pass an empty string to clear")]
    pub(super) due_date: Option<String>,
    #[arg(long, help = "Parent issue ID; pass an empty string to clear")]
    pub(super) parent: Option<String>,
    #[arg(long, help = "Stage ordinal (>=1) for this sub-issue")]
    pub(super) stage: Option<i64>,
    #[arg(long, help = "Ordering position within the board column")]
    pub(super) position: Option<f64>,
    #[arg(long, help = "Apply the update without starting an agent run")]
    pub(super) no_start: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueAssignArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "Assignee name (member, agent, or team; fuzzy match)")]
    pub(super) to: Option<String>,
    #[arg(long, help = "Assignee UUID — member, agent, or team")]
    pub(super) to_id: Option<String>,
    #[arg(long, help = "Remove current assignee")]
    pub(super) unassign: bool,
    #[arg(long, help = "Assign ownership without starting an agent run")]
    pub(super) no_start: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueStatusArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(value_name = "STATUS")]
    pub(super) status: String,
    #[arg(long, help = "Change status without starting an agent run")]
    pub(super) no_start: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("target")
        .required(true)
        .multiple(false)
        .args(["before", "after", "top", "bottom"])
))]
pub(super) struct IssueReorderArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "Place directly above this issue")]
    pub(super) before: Option<String>,
    #[arg(long, help = "Place directly below this issue")]
    pub(super) after: Option<String>,
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        help = "Move to the top of the current status column"
    )]
    pub(super) top: Option<bool>,
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        help = "Move to the bottom of the current status column"
    )]
    pub(super) bottom: Option<bool>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
