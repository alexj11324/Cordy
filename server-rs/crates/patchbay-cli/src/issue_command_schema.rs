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
    #[command(
        name = "dependency-graph",
        about = "Inspect or atomically apply a dependency graph for an issue"
    )]
    DependencyGraph(IssueDependencyGraphArgs),
    #[command(about = "Create a new issue")]
    Create(IssueCreateArgs),
    #[command(about = "Update an issue")]
    Update(IssueUpdateArgs),
    #[command(
        name = "patrick-mutate",
        about = "Apply an audited, revision-guarded issue mutation as Patrick"
    )]
    PatrickMutate(IssuePatrickMutationArgs),
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
        about = "Chronological issue history — status, executor, and comments"
    )]
    Timeline(IssueTimelineArgs),
    #[command(about = "Manage custom property values on an issue")]
    Property(IssuePropertyArgs),
}

#[derive(Debug, Args)]
pub(super) struct IssueDependencyGraphArgs {
    #[command(subcommand)]
    pub(super) command: IssueDependencyGraphCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum IssueDependencyGraphCommand {
    #[command(about = "Get the persisted dependency graph for an issue")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Validate and atomically apply a typed dependency plan")]
    Apply(IssueDependencyGraphApplyArgs),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("plan_source")
        .required(true)
        .multiple(false)
        .args(["plan_file", "plan_stdin"])
))]
pub(super) struct IssueDependencyGraphApplyArgs {
    #[arg(value_name = "PARENT-ID")]
    pub(super) parent: String,
    #[arg(
        long,
        help = "Plan idempotency key; reuse it to safely replay the same plan"
    )]
    pub(super) idempotency_key: String,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the complete typed plan from a UTF-8 JSON file"
    )]
    pub(super) plan_file: Option<std::path::PathBuf>,
    #[arg(long, help = "Read the complete typed plan from stdin as UTF-8 JSON")]
    pub(super) plan_stdin: bool,
    #[arg(long, help = "Allow --plan-file outside the current working directory")]
    pub(super) allow_external_file: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
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
    #[arg(long, help = "Execution target name (agent or team; fuzzy match)")]
    pub(super) executor: Option<String>,
    #[arg(
        long,
        help = "Execution target UUID — agent or team (mutually exclusive with --executor)"
    )]
    pub(super) executor_id: Option<String>,
    #[arg(long, help = "Human owner name (workspace member; fuzzy match)")]
    pub(super) owner: Option<String>,
    #[arg(
        long,
        help = "Human owner UUID (mutually exclusive with --owner)"
    )]
    pub(super) owner_id: Option<String>,
    #[arg(long, help = "Reviewer name (member, agent, or team; fuzzy match)")]
    pub(super) reviewer: Option<String>,
    #[arg(
        long,
        help = "Reviewer UUID — member, agent, or team (mutually exclusive with --reviewer)"
    )]
    pub(super) reviewer_id: Option<String>,
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
    #[arg(long, help = "New execution target name (agent or team; fuzzy match)")]
    pub(super) executor: Option<String>,
    #[arg(long, help = "New execution target UUID — agent or team")]
    pub(super) executor_id: Option<String>,
    #[arg(long, help = "New human owner name (workspace member; fuzzy match)")]
    pub(super) owner: Option<String>,
    #[arg(long, help = "New human owner UUID")]
    pub(super) owner_id: Option<String>,
    #[arg(long, help = "New reviewer name (member, agent, or team; fuzzy match)")]
    pub(super) reviewer: Option<String>,
    #[arg(long, help = "New reviewer UUID — member, agent, or team")]
    pub(super) reviewer_id: Option<String>,
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
#[command(group(
    ArgGroup::new("changes_source")
        .required(true)
        .multiple(false)
        .args(["changes_json", "changes_file", "changes_stdin"])
))]
pub(super) struct IssuePatrickMutationArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "Expected current issue revision")]
    pub(super) expected_revision: i64,
    #[arg(long, help = "Human-readable reason recorded in the audit trail")]
    pub(super) change_reason: String,
    #[arg(long, help = "Idempotency/correlation UUID for this mutation")]
    pub(super) correlation_id: String,
    #[arg(long, help = "JSON object containing only the allowed issue fields")]
    pub(super) changes_json: Option<String>,
    #[arg(long, value_name = "PATH", help = "Read the changes JSON object from a file")]
    pub(super) changes_file: Option<std::path::PathBuf>,
    #[arg(long, help = "Read the changes JSON object from stdin")]
    pub(super) changes_stdin: bool,
    #[arg(long, help = "Task UUID that authorized this mutation")]
    pub(super) task_id: Option<String>,
    #[arg(long, help = "Run UUID that authorized this mutation")]
    pub(super) run_id: Option<String>,
    #[arg(long, help = "Observed Linear updatedAt timestamp (RFC3339)")]
    pub(super) linear_remote_updated_at: Option<String>,
    #[arg(long, help = "Observed Linear issue snapshot as a JSON object")]
    pub(super) linear_remote_snapshot: Option<String>,
    #[arg(long, help = "Allow --changes-file outside the current working directory")]
    pub(super) allow_external_file: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct IssueAssignArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "Owner or execution target name (fuzzy match)")]
    pub(super) to: Option<String>,
    #[arg(long, help = "Owner or execution target UUID")]
    pub(super) to_id: Option<String>,
    #[arg(long, help = "Remove the current owner and execution target")]
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
