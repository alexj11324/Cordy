//! Cordy CLI — incremental Rust replacement for `server/cmd/cordy`.
//!
//! The S10 migration deliberately registers only fully functional commands.
//! Shared configuration, API, error, and safe text-input behavior is ported
//! with each vertical slice rather than exposing placeholder command trees.

mod api;
pub mod config;
pub mod error;

use anyhow::{bail, Context, Result};
use api::{http_timeout, ApiClient, HttpError, NetworkError};
use chrono::{DateTime, FixedOffset};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use config::Environment;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use url::{form_urlencoded, Url};

pub const CLIENT_VERSION: &str = env!("CORDY_BUILD_VERSION");
pub const BUILD_COMMIT: &str = env!("CORDY_BUILD_COMMIT");
pub const BUILD_DATE: &str = env!("CORDY_BUILD_DATE");
pub const BUILD_GO_VERSION: &str = env!("CORDY_BUILD_GO_VERSION");
pub const BUILD_OS: &str = env!("CORDY_BUILD_OS");
pub const BUILD_ARCH: &str = env!("CORDY_BUILD_ARCH");
pub const ROOT_LONG_VERSION: &str = concat!(
    env!("CORDY_BUILD_VERSION"),
    " (commit: ",
    env!("CORDY_BUILD_COMMIT"),
    ", built: ",
    env!("CORDY_BUILD_DATE"),
    ")\ngo: ",
    env!("CORDY_BUILD_GO_VERSION"),
    ", os/arch: ",
    env!("CORDY_BUILD_OS"),
    "/",
    env!("CORDY_BUILD_ARCH")
);

#[derive(Debug, Parser)]
#[command(
    name = "cordy",
    version = CLIENT_VERSION,
    long_version = ROOT_LONG_VERSION,
    about = "Cordy CLI — local agent runtime and management tool",
    long_about = "Work seamlessly with Cordy from the command line."
)]
pub struct Cli {
    #[arg(long, global = true, help = "Cordy server URL (env: CORDY_SERVER_URL)")]
    server_url: Option<String>,
    #[arg(long, global = true, help = "Workspace ID (env: CORDY_WORKSPACE_ID)")]
    workspace_id: Option<String>,
    #[arg(
        long,
        global = true,
        default_value = "",
        help = "Configuration profile name (e.g. dev)"
    )]
    profile: String,
    #[arg(
        long,
        global = true,
        help = "Print full error details on failure (env: CORDY_DEBUG)"
    )]
    debug: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Work with issues")]
    Issue(IssueArgs),
    #[command(about = "Authenticate cordy with Cordy")]
    Auth(AuthArgs),
    #[command(about = "Manage configuration for cordy")]
    Config(ConfigArgs),
    #[command(about = "Work with your user account")]
    User(UserArgs),
    #[command(about = "Work with workspaces")]
    Workspace(WorkspaceArgs),
    #[command(about = "Work with issue labels")]
    Label(LabelArgs),
    #[command(about = "Print version information")]
    Version {
        #[arg(long, value_enum, default_value_t = VersionOutput::Text)]
        output: VersionOutput,
    },
}

#[derive(Debug, Args)]
struct IssueArgs {
    #[command(subcommand)]
    command: IssueCommand,
}

#[derive(Debug, Args)]
struct LabelArgs {
    #[command(subcommand)]
    command: LabelCommand,
}

#[derive(Debug, Subcommand)]
enum LabelCommand {
    #[command(about = "List labels in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
    },
    #[command(about = "Get label details")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a new label")]
    Create(LabelCreateArgs),
    #[command(about = "Update a label")]
    Update(LabelUpdateArgs),
    #[command(about = "Delete a label")]
    Delete {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
}

#[derive(Debug, Args)]
struct LabelCreateArgs {
    #[arg(long, help = "Label name (required)")]
    name: Option<String>,
    #[arg(long, help = "Hex color like #3b82f6 (required)")]
    color: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct LabelUpdateArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long, help = "New name")]
    name: Option<String>,
    #[arg(long, help = "New hex color")]
    color: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Subcommand)]
enum IssueCommand {
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

#[derive(Debug, Args)]
struct IssueCreateArgs {
    #[arg(long, help = "Issue title (required)")]
    title: Option<String>,
    #[arg(
        long,
        help = "Issue description (decodes \\n, \\r, \\t, \\\\; pipe via --description-stdin to preserve literal backslashes)"
    )]
    description: Option<String>,
    #[arg(
        long,
        help = "Read issue description from stdin (preserves multi-line content verbatim)"
    )]
    description_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read issue description from a UTF-8 file"
    )]
    description_file: Option<String>,
    #[arg(
        long,
        help = "Allow --description-file / --attachment outside the current working directory"
    )]
    allow_external_file: bool,
    #[arg(long, help = "Issue status")]
    status: Option<String>,
    #[arg(long, help = "Issue priority")]
    priority: Option<String>,
    #[arg(long, help = "Assignee name (member, agent, or squad; fuzzy match)")]
    assignee: Option<String>,
    #[arg(
        long,
        help = "Assignee UUID — member, agent, or squad (mutually exclusive with --assignee)"
    )]
    assignee_id: Option<String>,
    #[arg(long, help = "Parent issue ID")]
    parent: Option<String>,
    #[arg(
        long,
        help = "Stage ordinal (>=1) grouping this sub-issue into an ordered barrier group under its parent"
    )]
    stage: Option<i64>,
    #[arg(long, help = "Project ID")]
    project: Option<String>,
    #[arg(long, help = "Start date (calendar day, YYYY-MM-DD)")]
    start_date: Option<String>,
    #[arg(long, help = "Due date (calendar day, YYYY-MM-DD)")]
    due_date: Option<String>,
    #[arg(
        long,
        help = "Allow creating an issue even when an active duplicate exists"
    )]
    allow_duplicate: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
    #[arg(long, value_delimiter = ',', help = "File path(s) to attach")]
    attachment: Vec<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Existing attachment UUID(s) to bind"
    )]
    attachment_id: Vec<String>,
}

#[derive(Debug, Args)]
struct IssueUpdateArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long, help = "New title")]
    title: Option<String>,
    #[arg(
        long,
        help = "New description (decodes \\n, \\r, \\t, \\\\; pipe via --description-stdin to preserve literal backslashes)"
    )]
    description: Option<String>,
    #[arg(long, help = "Read new description from stdin")]
    description_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read new description from a UTF-8 file"
    )]
    description_file: Option<String>,
    #[arg(
        long,
        help = "Allow --description-file outside the current working directory"
    )]
    allow_external_file: bool,
    #[arg(long, help = "New status")]
    status: Option<String>,
    #[arg(long, help = "New priority")]
    priority: Option<String>,
    #[arg(
        long,
        help = "New assignee name (member, agent, or squad; fuzzy match)"
    )]
    assignee: Option<String>,
    #[arg(long, help = "New assignee UUID — member, agent, or squad")]
    assignee_id: Option<String>,
    #[arg(long, help = "Project ID; pass an empty string to clear")]
    project: Option<String>,
    #[arg(long, help = "New start date; pass an empty string to clear")]
    start_date: Option<String>,
    #[arg(long, help = "New due date; pass an empty string to clear")]
    due_date: Option<String>,
    #[arg(long, help = "Parent issue ID; pass an empty string to clear")]
    parent: Option<String>,
    #[arg(long, help = "Stage ordinal (>=1) for this sub-issue")]
    stage: Option<i64>,
    #[arg(long, help = "Ordering position within the board column")]
    position: Option<f64>,
    #[arg(long, help = "Apply the update without starting an agent run")]
    no_start: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueAssignArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long, help = "Assignee name (member, agent, or squad; fuzzy match)")]
    to: Option<String>,
    #[arg(long, help = "Assignee UUID — member, agent, or squad")]
    to_id: Option<String>,
    #[arg(long, help = "Remove current assignee")]
    unassign: bool,
    #[arg(long, help = "Assign ownership without starting an agent run")]
    no_start: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueStatusArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(value_name = "STATUS")]
    status: String,
    #[arg(long, help = "Change status without starting an agent run")]
    no_start: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("target")
        .required(true)
        .multiple(false)
        .args(["before", "after", "top", "bottom"])
))]
struct IssueReorderArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long, help = "Place directly above this issue")]
    before: Option<String>,
    #[arg(long, help = "Place directly below this issue")]
    after: Option<String>,
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        help = "Move to the top of the current status column"
    )]
    top: Option<bool>,
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        help = "Move to the bottom of the current status column"
    )]
    bottom: Option<bool>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueCommentArgs {
    #[command(subcommand)]
    command: IssueCommentCommand,
}

#[derive(Debug, Subcommand)]
enum IssueCommentCommand {
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
struct IssueCommentListArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    #[arg(long, help = "Only comments created after this RFC3339 timestamp")]
    since: Option<String>,
    #[arg(long, help = "Return the thread containing this comment UUID")]
    thread: Option<String>,
    #[arg(long, help = "Cap replies to the N most recent within --thread")]
    tail: Option<i64>,
    #[arg(long, help = "Return the N most recently active threads")]
    recent: Option<i64>,
    #[arg(long, help = "Only return top-level comments")]
    roots_only: bool,
    #[arg(long, help = "Drop redundant fields from JSON output")]
    compact: bool,
    #[arg(long, help = "Clip comment content to a short preview")]
    summary: bool,
    #[arg(long, help = "Return resolved threads without folding")]
    full: bool,
    #[arg(long, help = "Composite pagination timestamp cursor")]
    before: Option<String>,
    #[arg(long, help = "Composite pagination UUID cursor")]
    before_id: Option<String>,
}

#[derive(Debug, Args)]
struct IssueCommentAddArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(
        long,
        help = "Comment content (decodes \\n, \\r, \\t, \\\\; use stdin to preserve literal backslashes)"
    )]
    content: Option<String>,
    #[arg(long, help = "Read comment content from stdin")]
    content_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read comment content from a UTF-8 file"
    )]
    content_file: Option<String>,
    #[arg(
        long,
        help = "Allow content/attachment files outside the current workdir"
    )]
    allow_external_file: bool,
    #[arg(long, help = "Parent comment ID to reply under")]
    parent: Option<String>,
    #[arg(long, value_delimiter = ',', help = "File path(s) to attach")]
    attachment: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueCommentResolutionArgs {
    #[arg(value_name = "COMMENT-ID")]
    comment_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueRunsArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    #[arg(long, help = "Show full task UUIDs in table output")]
    full_id: bool,
}

#[derive(Debug, Args)]
struct IssueRunMessagesArgs {
    #[arg(value_name = "TASK-ID")]
    task_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
    #[arg(long, help = "Only return messages after this sequence number")]
    since: i64,
    #[arg(long, help = "Issue ID/key to scope short task ID prefix resolution")]
    issue: Option<String>,
}

#[derive(Debug, Args)]
struct IssueUsageArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueRerunArgs {
    #[arg(value_name = "ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueCancelTaskArgs {
    #[arg(value_name = "TASK-ID")]
    task_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
    #[arg(long, help = "Issue ID/key to scope short task ID prefix resolution")]
    issue: Option<String>,
}

#[derive(Debug, Args)]
struct IssueSearchArgs {
    #[arg(value_name = "QUERY")]
    query: String,
    #[arg(long, default_value_t = 20, help = "Maximum number of results")]
    limit: i64,
    #[arg(long, help = "Include done and cancelled issues")]
    include_closed: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueSubscriberArgs {
    #[command(subcommand)]
    command: IssueSubscriberCommand,
}

#[derive(Debug, Subcommand)]
enum IssueSubscriberCommand {
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
struct IssueSubscriberMutationArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(
        long,
        help = "Member or agent name (fuzzy match; defaults to the caller)"
    )]
    user: Option<String>,
    #[arg(long, help = "Member or agent UUID (mutually exclusive with --user)")]
    user_id: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueLabelArgs {
    #[command(subcommand)]
    command: IssueLabelCommand,
}

#[derive(Debug, Subcommand)]
enum IssueLabelCommand {
    #[command(about = "List labels on an issue")]
    List(IssueLabelListArgs),
    #[command(about = "Attach a label to an issue")]
    Add(IssueLabelMutationArgs),
    #[command(about = "Remove a label from an issue")]
    Remove(IssueLabelMutationArgs),
}

#[derive(Debug, Args)]
struct IssueLabelListArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    #[arg(long, help = "Show full UUIDs in table output")]
    full_id: bool,
}

#[derive(Debug, Args)]
struct IssueLabelMutationArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(value_name = "LABEL-ID")]
    label_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    #[arg(long, help = "Show full UUIDs in table output")]
    full_id: bool,
}

#[derive(Debug, Args)]
struct IssueMetadataArgs {
    #[command(subcommand)]
    command: IssueMetadataCommand,
}

#[derive(Debug, Subcommand)]
enum IssueMetadataCommand {
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
struct IssueMetadataListArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueMetadataKeyArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, help = "Metadata key (required)")]
    key: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueMetadataDeleteArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, help = "Metadata key (required)")]
    key: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueMetadataSetArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, help = "Metadata key (required)")]
    key: Option<String>,
    #[arg(long, help = "Metadata value (required)")]
    value: Option<String>,
    #[arg(long = "type", help = "Force value type: string, number, or bool")]
    value_type: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueTimelineArgs {
    #[arg(value_name = "ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    #[arg(long, help = "Drop comments and return activity records only")]
    activity_only: bool,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Only return activities with these actions (repeatable or comma-separated)"
    )]
    action: Vec<String>,
    #[arg(
        long,
        help = "Only return entries created after this RFC3339 timestamp"
    )]
    since: Option<String>,
    #[arg(
        long,
        default_value_t = 0,
        help = "Only return the N most recent entries"
    )]
    tail: i64,
    #[arg(long, help = "Show full UUIDs in table output")]
    full_id: bool,
}

#[derive(Debug, Args)]
struct IssuePropertyArgs {
    #[command(subcommand)]
    command: IssuePropertyCommand,
}

#[derive(Debug, Subcommand)]
enum IssuePropertyCommand {
    #[command(about = "List custom property values set on an issue")]
    List(IssuePropertyListArgs),
    #[command(about = "Set a custom property value on an issue")]
    Set(IssuePropertyMutationArgs),
    #[command(about = "Remove a custom property value from an issue")]
    Unset(IssuePropertyUnsetArgs),
}

#[derive(Debug, Args)]
struct IssuePropertyListArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssuePropertyMutationArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, help = "Property name or UUID (required)")]
    name: Option<String>,
    #[arg(
        long,
        help = "Property value (required; see --help for per-type forms)"
    )]
    value: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssuePropertyUnsetArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(long, help = "Property name or UUID (required)")]
    name: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssuePullRequestArgs {
    #[command(subcommand)]
    command: IssuePullRequestCommand,
}

#[derive(Debug, Subcommand)]
enum IssuePullRequestCommand {
    #[command(about = "Attach an existing GitHub pull request to an issue")]
    Attach(IssuePullRequestAttachArgs),
}

#[derive(Debug, Args)]
struct IssuePullRequestAttachArgs {
    #[arg(value_name = "ISSUE-ID")]
    issue_id: String,
    #[arg(
        long,
        help = "GitHub pull request URL: https://github.com/{owner}/{repo}/pull/{number}"
    )]
    url: String,
    #[arg(
        long,
        help = "Optional PR title, used only when the workspace has no GitHub App installed"
    )]
    title: Option<String>,
    #[arg(long, help = "Optional PR state: open, closed, merged, or draft")]
    state: Option<String>,
    #[arg(long, help = "Optional head branch name")]
    branch: Option<String>,
    #[arg(long, help = "Optional head commit SHA")]
    head_sha: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct IssueListArgs {
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
    #[arg(long, help = "Show full UUIDs in table output")]
    full_id: bool,
    #[arg(long, help = "Filter by status")]
    status: Option<String>,
    #[arg(long, help = "Filter by priority")]
    priority: Option<String>,
    #[arg(
        long,
        help = "Filter by assignee name (member, agent, or squad; fuzzy match)"
    )]
    assignee: Option<String>,
    #[arg(
        long,
        help = "Filter by assignee UUID — member, agent, or squad (mutually exclusive with --assignee)"
    )]
    assignee_id: Option<String>,
    #[arg(long, help = "Filter by project ID")]
    project: Option<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Filter by metadata key=value (repeatable; combined with AND). Value is JSON-parsed: 'true'/'false' → bool, numbers → number, otherwise string. Wrap as '\"42\"' to force a string when the value would otherwise sniff as a number."
    )]
    metadata: Vec<String>,
    #[arg(
        long,
        default_value_t = 50,
        help = "Maximum number of issues to return"
    )]
    limit: i64,
    #[arg(
        long,
        default_value_t = 0,
        help = "Number of issues to skip (for pagination)"
    )]
    offset: i64,
    #[arg(
        long,
        help = "Sort column: position (default, manual board order), title, created_at, start_date, due_date, priority"
    )]
    sort: Option<String>,
    #[arg(
        long,
        help = "Sort direction (asc or desc); requires --sort to be a non-position column (position is always ascending)"
    )]
    direction: Option<String>,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: Option<ConfigCommand>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    #[command(about = "Show current CLI configuration")]
    Show {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Set a CLI configuration value")]
    Set { key: String, value: String },
}

#[derive(Debug, Args)]
struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    #[command(about = "Show current authentication status")]
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Remove stored authentication token")]
    Logout,
}

#[derive(Debug, Args)]
struct UserArgs {
    #[command(subcommand)]
    command: UserCommand,
}

#[derive(Debug, Subcommand)]
enum UserCommand {
    #[command(about = "Get or update your personal profile")]
    Profile(ProfileArgs),
}

#[derive(Debug, Args)]
struct ProfileArgs {
    #[command(subcommand)]
    command: ProfileCommand,
}

#[derive(Debug, Args)]
struct WorkspaceArgs {
    #[command(subcommand)]
    command: WorkspaceCommand,
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommand {
    #[command(about = "List all workspaces you belong to")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
    },
    #[command(about = "Get workspace details")]
    Get {
        #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
        workspace: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(
        about = "Create a workspace",
        long_about = "Creates a new workspace and adds you as its owner. Both --name and --slug are required; the slug is permanent (lowercase letters, digits, and hyphens) and cannot be changed after creation.\n\nCreating a workspace does NOT change the current default workspace for this profile — run 'cordy workspace switch <slug>' afterward if you want subsequent commands to target the new workspace."
    )]
    Create(CreateWorkspaceArgs),
    #[command(about = "Update workspace metadata (admin/owner only)")]
    Update(UpdateWorkspaceArgs),
}

#[derive(Debug, Args)]
struct CreateWorkspaceArgs {
    #[arg(long, help = "Workspace name")]
    name: Option<String>,
    #[arg(long, help = "Workspace slug")]
    slug: Option<String>,
    #[arg(
        long,
        help = "Workspace description (decodes \\n, \\r, \\t, \\\\; use --description-stdin to preserve literal backslashes)"
    )]
    description: Option<String>,
    #[arg(
        long,
        help = "Read description from stdin (preserves multi-line content verbatim)"
    )]
    description_stdin: bool,
    #[arg(
        long,
        help = "Workspace context (decodes \\n, \\r, \\t, \\\\; use --context-stdin to preserve literal backslashes)"
    )]
    context: Option<String>,
    #[arg(
        long,
        help = "Read context from stdin (preserves multi-line content verbatim)"
    )]
    context_stdin: bool,
    #[arg(long, help = "Issue prefix (uppercased server-side)")]
    issue_prefix: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct UpdateWorkspaceArgs {
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    workspace: Option<String>,
    #[arg(long, help = "New workspace name")]
    name: Option<String>,
    #[arg(
        long,
        help = "New description; pass an empty value to clear (decodes \\n, \\r, \\t, \\\\; use stdin/file to preserve literal backslashes)"
    )]
    description: Option<String>,
    #[arg(
        long,
        help = "Read description from stdin (preserves multi-line content verbatim)"
    )]
    description_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read description from a UTF-8 file")]
    description_file: Option<PathBuf>,
    #[arg(
        long,
        help = "New context; pass an empty value to clear (decodes \\n, \\r, \\t, \\\\; use stdin/file to preserve literal backslashes)"
    )]
    context: Option<String>,
    #[arg(
        long,
        help = "Read context from stdin (preserves multi-line content verbatim)"
    )]
    context_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read context from a UTF-8 file")]
    context_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Allow description/context files outside the current working directory"
    )]
    allow_external_file: bool,
    #[arg(long, help = "New issue prefix (uppercased server-side)")]
    issue_prefix: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Subcommand)]
enum ProfileCommand {
    #[command(about = "Show your current user profile")]
    Get {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(
        about = "Update your user profile (currently: profile description)",
        long_about = "Set the personal profile description that gets injected into agent briefs as `## Requesting User`. Pass an empty value to clear it.\n\nPick the input mode that preserves your content:\n  --description \"...\"          inline (decodes \\n / \\t escapes)\n  --description-stdin           pipe a HEREDOC (preserves verbatim)\n  --description-file <path>     read a UTF-8 file (Windows-safe)"
    )]
    Update(UpdateProfileArgs),
}

#[derive(Debug, Args)]
struct UpdateProfileArgs {
    #[arg(
        long,
        help = "New profile description (decodes \\n, \\r, \\t, \\\\; use --description-stdin to preserve literal backslashes)"
    )]
    description: Option<String>,
    #[arg(
        long,
        help = "Read description from stdin (preserves multi-line content verbatim)"
    )]
    description_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read description from a UTF-8 file inside the current working directory"
    )]
    description_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Allow --description-file to read outside the current working directory"
    )]
    allow_external_file: bool,
    #[arg(
        long,
        help = "Clear the profile description (equivalent to --description \"\")"
    )]
    clear: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum VersionOutput {
    #[default]
    Text,
    Json,
}

#[derive(Debug)]
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
}

impl Cli {
    pub fn debug_enabled(&self, environment: &Environment) -> bool {
        self.debug
            || environment.trimmed("CORDY_DEBUG").is_some_and(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
    }
}

pub async fn run(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    run_with_input(cli, environment, &mut stdin).await
}

async fn run_with_input<R: Read>(
    cli: &Cli,
    environment: &Environment,
    input: &mut R,
) -> Result<RunOutput> {
    match &cli.command {
        Command::Issue(IssueArgs {
            command: IssueCommand::List(args),
        }) => run_issue_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Get { id, output },
        }) => run_issue_get(cli, environment, id, *output).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::PullRequests { id, output },
        }) => run_issue_pull_requests(cli, environment, id, *output).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::PullRequest(IssuePullRequestArgs {
                    command: IssuePullRequestCommand::Attach(args),
                }),
        }) => run_issue_pull_request_attach(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Children {
                    id,
                    output,
                    full_id,
                },
        }) => run_issue_children(cli, environment, id, *output, *full_id).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Create(args),
        }) => run_issue_create(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Update(args),
        }) => run_issue_update(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Assign(args),
        }) => run_issue_assign(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Status(args),
        }) => run_issue_status(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Reorder(args),
        }) => run_issue_reorder(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::List(args),
                }),
        }) => run_issue_comment_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Add(args),
                }),
        }) => run_issue_comment_add(cli, environment, args, input).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Delete { comment_id },
                }),
        }) => run_issue_comment_delete(cli, environment, comment_id).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Resolve(args),
                }),
        }) => run_issue_comment_resolution(cli, environment, args, true).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Comment(IssueCommentArgs {
                    command: IssueCommentCommand::Unresolve(args),
                }),
        }) => run_issue_comment_resolution(cli, environment, args, false).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Runs(args),
        }) => run_issue_runs(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::RunMessages(args),
        }) => run_issue_run_messages(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Usage(args),
        }) => run_issue_usage(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Rerun(args),
        }) => run_issue_rerun(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::CancelTask(args),
        }) => run_issue_cancel_task(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Search(args),
        }) => run_issue_search(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::List { issue_id, output },
                }),
        }) => run_issue_subscriber_list(cli, environment, issue_id, *output).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Add(args),
                }),
        }) => run_issue_subscriber_mutation(cli, environment, args, true).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Remove(args),
                }),
        }) => run_issue_subscriber_mutation(cli, environment, args, false).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::List(args),
                }),
        }) => run_issue_label_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Add(args),
                }),
        }) => run_issue_label_add(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Remove(args),
                }),
        }) => run_issue_label_remove(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::List(args),
                }),
        }) => run_issue_metadata_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Get(args),
                }),
        }) => run_issue_metadata_get(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Set(args),
                }),
        }) => run_issue_metadata_set(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Delete(args),
                }),
        }) => run_issue_metadata_delete(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Timeline(args),
        }) => run_issue_timeline(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::List(args),
                }),
        }) => run_issue_property_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Set(args),
                }),
        }) => run_issue_property_set(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Unset(args),
                }),
        }) => run_issue_property_unset(cli, environment, args).await,
        Command::Auth(AuthArgs {
            command: AuthCommand::Status { output },
        }) => run_auth_status(cli, environment, *output).await,
        Command::Auth(AuthArgs {
            command: AuthCommand::Logout,
        }) => run_auth_logout(cli, environment),
        Command::Config(ConfigArgs { command: None }) => {
            run_config_show(cli, environment, OutputFormat::Table)
        }
        Command::Config(ConfigArgs {
            command: Some(ConfigCommand::Show { output }),
        }) => run_config_show(cli, environment, *output),
        Command::Config(ConfigArgs {
            command: Some(ConfigCommand::Set { key, value }),
        }) => run_config_set(cli, environment, key, value),
        Command::User(UserArgs {
            command:
                UserCommand::Profile(ProfileArgs {
                    command: ProfileCommand::Get { output },
                }),
        }) => run_user_profile_get(cli, environment, *output).await,
        Command::User(UserArgs {
            command:
                UserCommand::Profile(ProfileArgs {
                    command: ProfileCommand::Update(args),
                }),
        }) => run_user_profile_update(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::List { output, full_id },
        }) => run_workspace_list(cli, environment, *output, *full_id).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Get { workspace, output },
        }) => run_workspace_get(cli, environment, workspace.as_deref(), *output).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Create(args),
        }) => run_workspace_create(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Update(args),
        }) => run_workspace_update(cli, environment, args, input).await,
        Command::Label(LabelArgs {
            command: LabelCommand::List { output, full_id },
        }) => run_label_list(cli, environment, *output, *full_id).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Get { id, output },
        }) => run_label_get(cli, environment, id, *output).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Create(args),
        }) => run_label_create(cli, environment, args).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Update(args),
        }) => run_label_update(cli, environment, args).await,
        Command::Label(LabelArgs {
            command: LabelCommand::Delete { id, output },
        }) => run_label_delete(cli, environment, id, *output).await,
        Command::Version { output } => run_version(*output),
    }
}

fn run_version(output: VersionOutput) -> Result<RunOutput> {
    let stdout = match output {
        VersionOutput::Text => format!(
            "cordy {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})\ngo: {BUILD_GO_VERSION}, os/arch: {BUILD_OS}/{BUILD_ARCH}\n"
        ),
        VersionOutput::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "version": CLIENT_VERSION,
                "commit": BUILD_COMMIT,
                "date": BUILD_DATE,
                "go": BUILD_GO_VERSION,
                "os": BUILD_OS,
                "arch": BUILD_ARCH
            }))?
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct AuthUser {
    name: String,
    email: String,
}

async fn run_auth_status(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    require_task_local_config_root(environment)?;
    let task_context = environment.in_daemon_managed_execution_context();
    let (server_url, token) = resolve_auth_status_credentials(cli, environment)?;
    if token.is_empty() {
        return Ok(match output {
            OutputFormat::Table => RunOutput {
                stdout: String::new(),
                stderr: "Not authenticated. Run 'cordy login' to authenticate.\n".into(),
            },
            OutputFormat::Json => RunOutput {
                stdout: format!(
                    "{}\n",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "authenticated": false,
                        "server": server_url
                    }))?
                ),
                stderr: String::new(),
            },
        });
    }

    let client = ApiClient::new(
        server_url.clone(),
        String::new(),
        token.clone(),
        String::new(),
        String::new(),
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")),
        CLIENT_VERSION,
    )?;
    let user = match client.get_json::<AuthUser>("/api/me").await {
        Ok(user) => user,
        Err(error) => {
            let message = format!(
                "Token is invalid or expired: {error}\nRun 'cordy login' to re-authenticate."
            );
            return Ok(match output {
                OutputFormat::Table => RunOutput {
                    stdout: String::new(),
                    stderr: format!("{message}\n"),
                },
                OutputFormat::Json => RunOutput {
                    stdout: format!(
                        "{}\n",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "authenticated": false,
                            "server": server_url,
                            "error": message
                        }))?
                    ),
                    stderr: String::new(),
                },
            });
        }
    };
    let token_prefix = display_token_prefix(&token);
    Ok(match output {
        OutputFormat::Table => RunOutput {
            stdout: String::new(),
            stderr: if task_context {
                format!(
                    "Server:  {server_url}\nUser:    {} ({})\n",
                    user.name, user.email
                )
            } else {
                format!(
                    "Server:  {server_url}\nUser:    {} ({})\nToken:   {token_prefix}\n",
                    user.name, user.email
                )
            },
        },
        OutputFormat::Json => {
            let mut status = serde_json::json!({
                "authenticated": true,
                "server": server_url,
                "user": user
            });
            if !task_context {
                status["token"] = Value::String(token_prefix);
            }
            RunOutput {
                stdout: format!("{}\n", serde_json::to_string_pretty(&status)?),
                stderr: String::new(),
            }
        }
    })
}

fn run_auth_logout(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    require_human_local_command(environment, "logout")?;
    let removed = environment
        .clear_profile_token(&cli.profile)
        .context("failed to save config")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: if removed {
            "Token removed. You are now logged out.\n".into()
        } else {
            "Not authenticated.\n".into()
        },
    })
}

fn require_task_local_config_root(environment: &Environment) -> Result<()> {
    if !environment.in_daemon_managed_execution_context()
        || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some()
    {
        return Ok(());
    }
    let suffix = environment
        .leftover_marker_suffix()
        .unwrap_or_else(|| environment.daemon_port_only_context_hint().into());
    bail!(
        "daemon-managed task requires a task-local Cordy config root in {}{suffix}",
        config::TASK_CONFIG_ROOT_ENV
    )
}

fn require_human_local_command(environment: &Environment, command: &str) -> Result<()> {
    if !environment.in_daemon_task_identity_context() {
        return Ok(());
    }
    let suffix = environment.leftover_marker_suffix().unwrap_or_default();
    bail!("{command} is not available inside a daemon-managed task{suffix}")
}

fn resolve_auth_status_credentials(
    cli: &Cli,
    environment: &Environment,
) -> Result<(String, String)> {
    let task_context = environment.in_daemon_managed_execution_context();
    let may_read_config =
        !task_context || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some();
    let config = if may_read_config {
        environment.load_config(&cli.profile).unwrap_or_default()
    } else {
        config::CliConfig::default()
    };
    let token = environment
        .trimmed("CORDY_TOKEN")
        .map(ToOwned::to_owned)
        .or_else(|| (!task_context).then(|| config.token.clone()))
        .unwrap_or_default();
    if task_context && !token.starts_with("mat_") {
        bail!("agent execution context requires CORDY_TOKEN to be a task-scoped mat_ token");
    }
    let explicit_server_url = cli
        .server_url
        .as_deref()
        .or_else(|| environment.trimmed("CORDY_SERVER_URL"));
    let server_url = if let Some(raw) = explicit_server_url.filter(|value| !value.is_empty()) {
        normalize_api_base_url(raw).unwrap_or_else(|_| raw.into())
    } else if may_read_config && !config.server_url.is_empty() {
        normalize_api_base_url(&config.server_url).unwrap_or(config.server_url)
    } else {
        String::new()
    };
    if server_url.is_empty() {
        bail!(
            "No server configured. Run 'cordy setup' first{}.",
            environment.daemon_port_only_context_hint()
        );
    }
    Ok((server_url, token))
}

fn display_token_prefix(token: &str) -> String {
    if token.chars().count() > 12 {
        token.chars().take(12).collect::<String>() + "..."
    } else {
        token.into()
    }
}

const CONFIG_SET_SUPPORTED_KEYS: &[&str] = &[
    "server_url",
    "app_url",
    "workspace_id",
    "device_name",
    "runtime_name",
    "workspaces_root",
    "max_concurrent_tasks",
    "poll_interval",
    "heartbeat_interval",
    "agent_timeout",
    "codex_semantic_inactivity_timeout",
    "codex_handshake_timeout",
    "disable_auto_update",
    "auto_update_check_interval",
    "disable_auto_reload",
];

fn run_config_show(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    require_task_local_config_root(environment)?;
    let path = environment.config_path(&cli.profile)?;
    let document = environment.load_profile_document(&cli.profile)?;
    let values = config_display_values(&document)?;
    let stdout = match output {
        OutputFormat::Table => format_config_table(&path, &cli.profile, &values),
        OutputFormat::Json => {
            let mut object = serde_json::Map::new();
            object.insert(
                "config_file".into(),
                Value::String(path.display().to_string()),
            );
            if !cli.profile.is_empty() {
                object.insert("profile".into(), Value::String(cli.profile.clone()));
            }
            for (key, value) in values {
                object.insert(key.into(), value);
            }
            format!(
                "{}\n",
                serde_json::to_string_pretty(&Value::Object(object))?
            )
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn run_config_set(
    cli: &Cli,
    environment: &Environment,
    key: &str,
    value: &str,
) -> Result<RunOutput> {
    require_task_local_config_root(environment)?;
    let (stored, displayed) = validate_config_set(key, value, environment)?;
    environment.set_profile_value(&cli.profile, key, stored)?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Set {key} = {displayed}\n"),
    })
}

fn config_display_values(document: &Value) -> Result<Vec<(&'static str, Value)>> {
    let object = document
        .as_object()
        .context("parse CLI config: expected a JSON object")?;
    let string = |key: &'static str| -> Result<Value> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Value::Null),
            Some(Value::String(value)) if value.is_empty() => Ok(Value::Null),
            Some(Value::String(value)) => Ok(Value::String(value.clone())),
            Some(_) => bail!("parse CLI config: field {key:?} must be a string"),
        }
    };
    let integer = |key: &'static str| -> Result<Value> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Value::Null),
            Some(Value::Number(value)) if value.as_i64() == Some(0) => Ok(Value::Null),
            Some(Value::Number(value)) if value.as_i64().is_some() => {
                Ok(Value::Number(value.clone()))
            }
            Some(_) => bail!("parse CLI config: field {key:?} must be an integer"),
        }
    };
    let boolean = |key: &'static str| -> Result<Value> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Value::Bool(false)),
            Some(Value::Bool(value)) => Ok(Value::Bool(*value)),
            Some(_) => bail!("parse CLI config: field {key:?} must be a boolean"),
        }
    };
    Ok(vec![
        ("server_url", string("server_url")?),
        ("app_url", string("app_url")?),
        ("workspace_id", string("workspace_id")?),
        ("device_name", string("device_name")?),
        ("runtime_name", string("runtime_name")?),
        ("workspaces_root", string("workspaces_root")?),
        ("max_concurrent_tasks", integer("max_concurrent_tasks")?),
        ("poll_interval", string("poll_interval")?),
        ("heartbeat_interval", string("heartbeat_interval")?),
        ("agent_timeout", string("agent_timeout")?),
        (
            "codex_semantic_inactivity_timeout",
            string("codex_semantic_inactivity_timeout")?,
        ),
        (
            "codex_handshake_timeout",
            string("codex_handshake_timeout")?,
        ),
        ("disable_auto_update", boolean("disable_auto_update")?),
        (
            "auto_update_check_interval",
            string("auto_update_check_interval")?,
        ),
        ("disable_auto_reload", boolean("disable_auto_reload")?),
    ])
}

fn format_config_table(path: &Path, profile: &str, values: &[(&str, Value)]) -> String {
    let mut output = format!("Config file: {}\n", path.display());
    if !profile.is_empty() {
        let _ = writeln!(output, "Profile:      {profile}");
    }
    for (key, value) in values {
        let rendered = match (*key, value) {
            ("agent_timeout", Value::String(value))
                if parse_go_duration(value).is_some_and(|duration| duration == 0.0) =>
            {
                format!("{value} (disabled)")
            }
            (_, Value::String(value)) => value.clone(),
            (_, Value::Bool(value)) => value.to_string(),
            (_, Value::Number(value)) => value.to_string(),
            _ => "(not set)".into(),
        };
        let label = format!("{key}:");
        let _ = writeln!(output, "{label:<34} {rendered}");
    }
    output
}

fn validate_config_set(
    key: &str,
    value: &str,
    environment: &Environment,
) -> Result<(Option<Value>, String)> {
    let clear = || (None, String::new());
    match key {
        "server_url" => validate_url_config(value, key, &["http", "https", "ws", "wss"]),
        "app_url" => validate_url_config(value, key, &["http", "https"]),
        "workspace_id" | "device_name" | "runtime_name" => Ok(if value.is_empty() {
            clear()
        } else {
            (Some(Value::String(value.into())), value.into())
        }),
        "workspaces_root" => {
            let value = value.trim();
            if value.is_empty() {
                return Ok(clear());
            }
            let path = Path::new(value);
            let absolute = if path.is_absolute() {
                lexical_normalize(path)
            } else {
                lexical_normalize(&environment.current_dir().join(path))
            };
            let value = absolute.display().to_string();
            Ok((Some(Value::String(value.clone())), value))
        }
        "max_concurrent_tasks" => {
            if value.is_empty() {
                return Ok(clear());
            }
            let number = value.parse::<i64>().with_context(|| {
                format!("max_concurrent_tasks must be an integer: invalid value {value:?}")
            })?;
            if number < 0 {
                bail!("max_concurrent_tasks must be >= 0 (got {number})");
            }
            Ok(if number == 0 {
                clear()
            } else {
                (Some(Value::Number(number.into())), value.into())
            })
        }
        "poll_interval" => validate_positive_duration(key, value, false),
        "heartbeat_interval"
        | "codex_semantic_inactivity_timeout"
        | "codex_handshake_timeout"
        | "auto_update_check_interval" => validate_positive_duration(key, value, true),
        "agent_timeout" => {
            if value.is_empty() {
                return Ok(clear());
            }
            let duration = parse_go_duration(value).with_context(|| {
                format!(
                    "agent_timeout must be a Go duration (e.g. 10m, 0s to disable): invalid value {value:?}"
                )
            })?;
            if duration < 0.0 {
                bail!(
                    "agent_timeout must be >= 0 (got {value}); use 0s to disable the cap or \"\" to clear the persisted value"
                );
            }
            Ok((Some(Value::String(value.into())), value.into()))
        }
        "disable_auto_update" | "disable_auto_reload" => {
            if value.is_empty() {
                return Ok(clear());
            }
            let parsed = parse_go_bool(value)
                .with_context(|| format!("{key} must be 'true' or 'false' (got {value:?})"))?;
            Ok(if parsed {
                (Some(Value::Bool(true)), value.into())
            } else {
                clear()
            })
        }
        _ => bail!(
            "unknown config key {key:?} (supported: {})",
            CONFIG_SET_SUPPORTED_KEYS.join(", ")
        ),
    }
}

fn validate_url_config(
    value: &str,
    key: &str,
    schemes: &[&str],
) -> Result<(Option<Value>, String)> {
    if value.is_empty() {
        return Ok((None, String::new()));
    }
    let url = Url::parse(value).with_context(|| format!("{key} must be a valid URL"))?;
    if url.host_str().is_none() {
        bail!("{key} must be a valid URL with a host");
    }
    if !schemes.contains(&url.scheme()) {
        bail!("{key} must use one of: {}", schemes.join(", "));
    }
    Ok((Some(Value::String(value.into())), value.into()))
}

fn validate_positive_duration(
    key: &str,
    value: &str,
    trim: bool,
) -> Result<(Option<Value>, String)> {
    if value.is_empty() {
        return Ok((None, String::new()));
    }
    let stored = if trim { value.trim() } else { value };
    let duration = parse_go_duration(stored).with_context(|| {
        format!("{key} must be a Go duration (e.g. 10s, 500ms): invalid value {value:?}")
    })?;
    if duration <= 0.0 {
        bail!("{key} must be positive (got {stored}); use `config set {key} \"\"` to clear it");
    }
    Ok((Some(Value::String(stored.into())), stored.into()))
}

fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

fn parse_go_duration(value: &str) -> Option<f64> {
    if value.is_empty() || value.trim() != value {
        return None;
    }
    let (sign, mut rest) = match value.as_bytes().first() {
        Some(b'-') => (-1.0, &value[1..]),
        Some(b'+') => (1.0, &value[1..]),
        _ => (1.0, value),
    };
    if rest.is_empty() {
        return None;
    }
    if rest == "0" {
        return Some(0.0 * sign);
    }
    let mut seconds = 0.0_f64;
    while !rest.is_empty() {
        let number_len = rest
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit() || *character == '.')
            .map(|(index, character)| index + character.len_utf8())
            .last()?;
        let number = rest[..number_len].parse::<f64>().ok()?;
        rest = &rest[number_len..];
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        rest = &rest[unit.len()..];
        seconds += number * multiplier;
    }
    const MAX_GO_DURATION_SECONDS: f64 = i64::MAX as f64 / 1_000_000_000.0;
    (seconds.is_finite() && seconds <= MAX_GO_DURATION_SECONDS).then_some(sign * seconds)
}

const VALID_ISSUE_SORT_COLUMNS: &[&str] = &[
    "position",
    "title",
    "created_at",
    "start_date",
    "due_date",
    "priority",
];

#[derive(Debug, Default, Deserialize)]
struct IssueListResponse {
    #[serde(default)]
    issues: Value,
    #[serde(default)]
    total: Value,
}

#[derive(Debug, Serialize)]
struct IssueListEnvelope<'a> {
    has_more: bool,
    issues: &'a [Value],
    limit: i64,
    offset: i64,
    total: i64,
}

async fn run_issue_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if workspace_id.is_empty() {
        if environment.in_daemon_managed_execution_context() {
            bail!(
                "workspace_id is required: CORDY_WORKSPACE_ID must be set by the daemon in agent execution context (no fallback to user config)"
            );
        }
        bail!(
            "workspace_id is required: use --workspace-id flag, set CORDY_WORKSPACE_ID env, or run 'cordy config set workspace_id <id>'"
        );
    }

    let query = build_issue_list_query(&client, &workspace_id, args).await?;
    let path = format!("/api/issues?{query}");
    let result: IssueListResponse = client.get_json(&path).await.context("list issues")?;
    let issues = result.issues.as_array().cloned().unwrap_or_default();
    let total = result.total.as_f64().unwrap_or_default() as i64;

    let stdout = match args.output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&IssueListEnvelope {
                has_more: issue_list_has_more(args.offset, issues.len(), total),
                issues: &issues,
                limit: args.limit,
                offset: args.offset,
                total,
            })?
        ),
        OutputFormat::Table => {
            let actors = load_issue_actor_names(&client, &workspace_id, &issues).await;
            format_issue_list_table(&issues, args.full_id, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn issue_list_has_more(offset: i64, issue_count: usize, total: i64) -> bool {
    offset + (issue_count as i64) < total
}

async fn build_issue_list_query(
    client: &ApiClient,
    workspace_id: &str,
    args: &IssueListArgs,
) -> Result<String> {
    let mut params = BTreeMap::<String, String>::new();
    params.insert("workspace_id".into(), workspace_id.into());
    if let Some(status) = args.status.as_deref().filter(|value| !value.is_empty()) {
        params.insert("status".into(), status.into());
    }
    if let Some(priority) = args.priority.as_deref().filter(|value| !value.is_empty()) {
        params.insert("priority".into(), priority.into());
    }
    if args.limit > 0 {
        params.insert("limit".into(), args.limit.to_string());
    }
    if args.offset > 0 {
        params.insert("offset".into(), args.offset.to_string());
    }

    if args.assignee.is_some() && args.assignee_id.is_some() {
        bail!("--assignee and --assignee-id are mutually exclusive");
    }
    if let Some(id) = &args.assignee_id {
        let assignee = resolve_issue_assignee_id(client, workspace_id, id)
            .await
            .context("resolve assignee")?;
        params.insert("assignee_id".into(), assignee.id);
    } else if let Some(name) = &args.assignee {
        let assignee = resolve_issue_assignee_name(client, workspace_id, name)
            .await
            .context("resolve assignee")?;
        params.insert("assignee_id".into(), assignee.id);
    }

    if let Some(project) = args.project.as_deref().filter(|value| !value.is_empty()) {
        params.insert(
            "project_id".into(),
            resolve_issue_project_id(client, workspace_id, project).await?,
        );
    }
    if !args.metadata.is_empty() {
        params.insert("metadata".into(), build_metadata_filter(&args.metadata)?);
    }
    if let Some(sort) = args.sort.as_deref().filter(|value| !value.is_empty()) {
        if !VALID_ISSUE_SORT_COLUMNS.contains(&sort) {
            bail!(
                "invalid --sort {sort:?}; valid values: {}",
                VALID_ISSUE_SORT_COLUMNS.join(", ")
            );
        }
        params.insert("sort".into(), sort.into());
    }
    if let Some(direction) = args.direction.as_deref().filter(|value| !value.is_empty()) {
        let direction = direction.to_ascii_lowercase();
        if direction != "asc" && direction != "desc" {
            bail!(
                "invalid --direction {:?}; valid values: asc, desc",
                args.direction.as_deref().unwrap_or_default()
            );
        }
        if matches!(args.sort.as_deref(), None | Some("") | Some("position")) {
            bail!(
                "--direction requires --sort to be one of title, created_at, start_date, due_date, priority; position (the default manual board order) is always ascending"
            );
        }
        params.insert("direction".into(), direction);
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(&key, &value);
    }
    Ok(serializer.finish())
}

fn build_metadata_filter(pairs: &[String]) -> Result<String> {
    let mut values = BTreeMap::<String, Value>::new();
    for pair in pairs {
        let Some((key, raw)) = pair.split_once('=') else {
            bail!("--metadata {pair:?} must be in key=value form");
        };
        if key.is_empty() {
            bail!("--metadata {pair:?} must be in key=value form");
        }
        if values.contains_key(key) {
            bail!("--metadata key {key:?} given more than once; combine into a single filter");
        }
        let parsed = serde_json::from_str::<Value>(raw).ok();
        let value = match parsed {
            Some(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => value,
            _ => Value::String(raw.into()),
        };
        values.insert(key.into(), value);
    }
    serde_json::to_string(&values).context("encode metadata filter")
}

#[derive(Clone, Debug)]
struct IssueActor {
    actor_type: &'static str,
    id: String,
    name: String,
    email: String,
    archived: bool,
}

#[derive(Debug)]
struct ResolvedIssueAssignee {
    actor_type: String,
    id: String,
    name: String,
}

async fn fetch_issue_actors(
    client: &ApiClient,
    workspace_id: &str,
    include_squads: bool,
) -> [Result<Vec<IssueActor>>; 3] {
    let members =
        retry_actor_get::<Vec<Value>>(client, &format!("/api/workspaces/{workspace_id}/members"))
            .await
            .map(|items| {
                items
                    .iter()
                    .map(|item| IssueActor {
                        actor_type: "member",
                        id: value_string(item, "user_id"),
                        name: value_string(item, "name"),
                        email: value_string(item, "email"),
                        archived: false,
                    })
                    .collect()
            });
    let agents = retry_actor_get::<Vec<Value>>(
        client,
        &format!(
            "/api/agents?workspace_id={}",
            form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
        ),
    )
    .await
    .map(|items| {
        items
            .iter()
            .map(|item| IssueActor {
                actor_type: "agent",
                id: value_string(item, "id"),
                name: value_string(item, "name"),
                email: String::new(),
                archived: false,
            })
            .collect()
    });
    let squads = if include_squads {
        retry_actor_get::<Vec<Value>>(client, "/api/squads")
            .await
            .map(|items| {
                items
                    .iter()
                    .map(|item| IssueActor {
                        actor_type: "squad",
                        id: value_string(item, "id"),
                        name: value_string(item, "name"),
                        email: String::new(),
                        archived: !value_string(item, "archived_at").is_empty(),
                    })
                    .collect()
            })
    } else {
        Ok(Vec::new())
    };
    [members, agents, squads]
}

async fn retry_actor_get<T: DeserializeOwned>(client: &ApiClient, path: &str) -> Result<T> {
    let delays = [100_u64, 250];
    for (attempt, delay) in [0_u64, 100, 250].into_iter().enumerate() {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        match client.get_json(path).await {
            Ok(value) => return Ok(value),
            Err(error)
                if error.downcast_ref::<NetworkError>().is_some() && attempt < delays.len() => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("actor resolver retry loop always returns")
}

async fn resolve_issue_assignee_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_id(client, workspace_id, raw, true).await
}

async fn resolve_subscriber_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_id(client, workspace_id, raw, false).await
}

async fn resolve_actor_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
    allow_squads: bool,
) -> Result<ResolvedIssueAssignee> {
    let input = raw.trim();
    if !is_canonical_uuid(input) {
        bail!("expected a canonical UUID, got {raw:?}");
    }
    let actors = fetch_issue_actors(client, workspace_id, allow_squads).await;
    let actor_kind_count = if allow_squads { 3 } else { 2 };
    if actors[..actor_kind_count].iter().all(Result::is_err) {
        let errors = actors[..actor_kind_count]
            .iter()
            .enumerate()
            .map(|(index, result)| {
                let kind = ["members", "agents", "squads"][index];
                format!("fetch {kind}: {}", result.as_ref().unwrap_err())
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !allow_squads {
            bail!("failed to resolve user: {errors}");
        }
        bail!(
            "failed to resolve assignee: {}; {}; {}",
            actors[0].as_ref().unwrap_err(),
            actors[1].as_ref().unwrap_err(),
            actors[2].as_ref().unwrap_err()
        );
    }
    if let Some(actor) = actors
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flatten()
        .find(|actor| {
            (allow_squads || actor.actor_type != "squad") && actor.id.eq_ignore_ascii_case(input)
        })
    {
        return Ok(ResolvedIssueAssignee {
            actor_type: actor.actor_type.into(),
            id: actor.id.clone(),
            name: actor.name.clone(),
        });
    }
    if allow_squads {
        bail!("no member, agent, or squad found with ID {input:?}")
    }
    bail!("no member or agent found with ID {input:?}")
}

async fn resolve_issue_assignee_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_name(client, workspace_id, raw, true).await
}

async fn resolve_subscriber_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_name(client, workspace_id, raw, false).await
}

async fn resolve_actor_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
    allow_squads: bool,
) -> Result<ResolvedIssueAssignee> {
    let input = normalize_assignee_input(raw);
    if input.is_empty() {
        if allow_squads {
            bail!("no member, agent, or squad found matching {raw:?}");
        }
        bail!("no member or agent found matching {raw:?}");
    }
    let actors = fetch_issue_actors(client, workspace_id, allow_squads).await;
    let actor_kind_count = if allow_squads { 3 } else { 2 };
    if actors[..actor_kind_count].iter().all(Result::is_err) {
        let errors = actors[..actor_kind_count]
            .iter()
            .enumerate()
            .map(|(index, result)| {
                let kind = ["members", "agents", "squads"][index];
                format!("fetch {kind}: {}", result.as_ref().unwrap_err())
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !allow_squads {
            bail!("failed to resolve user: {errors}");
        }
        bail!("failed to resolve assignee: {errors}");
    }
    let actors = actors
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flatten()
        .filter(|actor| !actor.archived && (allow_squads || actor.actor_type != "squad"))
        .collect::<Vec<_>>();
    let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
    for actor in actors {
        let short_id = display_id(&actor.id, false);
        if actor.id.eq_ignore_ascii_case(&input)
            || short_id.eq_ignore_ascii_case(&input)
            || (!actor.email.is_empty() && actor.email.eq_ignore_ascii_case(&input))
        {
            buckets[0].push(actor);
        } else if actor.name.eq_ignore_ascii_case(&input) {
            buckets[1].push(actor);
        } else if actor
            .name
            .to_ascii_lowercase()
            .contains(&input.to_ascii_lowercase())
        {
            buckets[2].push(actor);
        }
    }
    for bucket in buckets {
        match bucket.as_slice() {
            [] => {}
            [actor] => {
                return Ok(ResolvedIssueAssignee {
                    actor_type: actor.actor_type.into(),
                    id: actor.id.clone(),
                    name: actor.name.clone(),
                });
            }
            actors => {
                let matches = actors
                    .iter()
                    .map(|actor| {
                        format!(
                            "  {} {:?} ({})",
                            actor.actor_type,
                            actor.name,
                            display_id(&actor.id, false)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!("ambiguous assignee {input:?}; matches:\n{matches}");
            }
        }
    }
    if allow_squads {
        bail!("no member, agent, or squad found matching {input:?}")
    }
    bail!("no member or agent found matching {input:?}")
}

fn normalize_assignee_input(raw: &str) -> String {
    let input = raw.trim();
    if let Some(marker) = input.find("](mention://") {
        if input.starts_with('[') && input.ends_with(')') {
            let target = &input[marker + 12..input.len() - 1];
            if let Some((kind, id)) = target.split_once('/') {
                if matches!(kind, "member" | "agent" | "squad") {
                    return id.into();
                }
            }
        }
    }
    input.trim_start_matches(['@', '＠']).trim().to_string()
}

async fn resolve_issue_project_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<String> {
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok(input.into());
    }
    let compact = input.replace('-', "").to_ascii_lowercase();
    if compact.len() < 4 {
        bail!("resolve project: expected a full UUID or at least 4 hex characters, got {raw:?}");
    }
    if !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!(
            "resolve project: expected a UUID prefix containing only hex characters, got {raw:?}"
        );
    }
    let path = format!(
        "/api/projects?workspace_id={}",
        form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
    );
    let result: Value = client.get_json(&path).await.context("resolve project")?;
    let mut candidates = result
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|project| compact_uuid(&value_string(project, "id")).starts_with(&compact))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|project| value_string(project, "id"));
    match candidates.as_slice() {
        [project] => Ok(value_string(project, "id")),
        [] => bail!(
            "no project found matching id prefix {raw:?}; run the list command with --full-id to copy the full UUID"
        ),
        projects => {
            let matches = projects
                .iter()
                .map(|project| format!("  {}", value_string(project, "id")))
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "ambiguous project id prefix {raw:?}; matches:\n{matches}\nUse more characters or run the list command with --full-id"
            )
        }
    }
}

#[derive(Debug, Default)]
struct IssueActorNames(HashMap<String, String>);

async fn load_issue_actor_names(
    client: &ApiClient,
    workspace_id: &str,
    issues: &[Value],
) -> IssueActorNames {
    let needed = issues
        .iter()
        .filter_map(|issue| issue.get("assignee_type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if needed.is_empty() || workspace_id.is_empty() {
        return IssueActorNames::default();
    }
    let mut names = HashMap::new();
    let paths = [
        (
            "member",
            format!("/api/workspaces/{workspace_id}/members"),
            "user_id",
        ),
        (
            "agent",
            format!(
                "/api/agents?workspace_id={}",
                form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
            ),
            "id",
        ),
        ("squad", "/api/squads".into(), "id"),
    ];
    for (actor_type, path, id_field) in paths {
        if !needed.contains(&actor_type) {
            continue;
        }
        if let Ok(items) = client.get_json::<Vec<Value>>(&path).await {
            for item in items {
                let id = value_string(&item, id_field);
                let name = value_string(&item, "name");
                if !id.is_empty() && !name.is_empty() {
                    names.insert(format!("{actor_type}:{id}"), name);
                }
            }
        }
    }
    IssueActorNames(names)
}

fn format_issue_list_table(issues: &[Value], full_id: bool, actors: &IssueActorNames) -> String {
    let mut rows = Vec::with_capacity(issues.len() + 1);
    let mut headers = vec![
        "KEY".into(),
        "TITLE".into(),
        "STATUS".into(),
        "PRIORITY".into(),
        "ASSIGNEE".into(),
        "START DATE".into(),
        "DUE DATE".into(),
    ];
    if full_id {
        headers.insert(1, "ID".into());
    }
    rows.push(headers);
    for issue in issues {
        let id = value_string(issue, "id");
        let key = match value_string(issue, "identifier") {
            value if value.is_empty() => id.clone(),
            value => value,
        };
        let actor_type = value_string(issue, "assignee_type");
        let actor_id = value_string(issue, "assignee_id");
        let assignee = if actor_type.is_empty() || actor_id.is_empty() {
            String::new()
        } else {
            let actor_key = format!("{actor_type}:{actor_id}");
            actors
                .0
                .get(&actor_key)
                .map_or_else(|| actor_key.clone(), |name| format!("{actor_type}:{name}"))
        };
        let date = |field| {
            value_string(issue, field)
                .chars()
                .take(10)
                .collect::<String>()
        };
        let mut row = vec![
            key,
            value_string(issue, "title"),
            value_string(issue, "status"),
            value_string(issue, "priority"),
            assignee,
            date("start_date"),
            date("due_date"),
        ];
        if full_id {
            row.insert(1, id);
        }
        rows.push(row);
    }
    format_table(&rows)
}

async fn run_issue_get(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let issue: Value = client
        .get_json(&format!("/api/issues/{issue_id}"))
        .await
        .context("get issue")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors =
                load_issue_actor_names(&client, &workspace_id, std::slice::from_ref(&issue)).await;
            format_issue_get_table(&issue, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn resolve_issue_ref(client: &ApiClient, input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("issue id is required");
    }
    if looks_like_issue_identifier(trimmed) || is_canonical_uuid(trimmed) {
        let issue: Value = client.get_json(&format!("/api/issues/{trimmed}")).await?;
        return Ok(value_string(&issue, "id"));
    }
    if normalize_uuid_prefix(trimmed).is_some() {
        bail!(
            "issue ref {input:?} looks like a short UUID prefix; short prefixes are no longer supported for issues. Use the issue key (e.g. MUL-123) shown by `cordy issue list`, or pass the full UUID (run a list command with --full-id to copy it)"
        );
    }
    bail!(
        "issue ref {input:?} is not a recognized issue reference; use the issue key (e.g. MUL-123) shown by `cordy issue list`, or pass the full UUID"
    )
}

async fn resolve_task_run_id(
    client: &ApiClient,
    issue_id: Option<&str>,
    input: &str,
) -> Result<String> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok(trimmed.into());
    }
    let Some(issue_id) = issue_id.filter(|value| !value.trim().is_empty()) else {
        bail!(
            "short task run prefixes require --issue <issue-id>; pass a full task UUID or run `cordy issue runs <issue-id> --full-id`"
        );
    };
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("resolve task run: id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve task run: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve task run: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let runs: Vec<Value> = client
        .get_json(&format!("/api/issues/{issue_id}/task-runs"))
        .await
        .context("resolve task run")?;
    let mut matches = runs
        .iter()
        .map(|run| value_string(run, "id"))
        .filter(|id| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!(
            "no task run found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous task run id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches.join("\n  ")
        ),
    }
}

async fn resolve_label_id(client: &ApiClient, workspace_id: &str, input: &str) -> Result<String> {
    resolve_label_reference(client, workspace_id, input)
        .await
        .map(|(id, _)| id)
}

async fn resolve_label_reference(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<(String, String)> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok((trimmed.into(), trimmed.into()));
    }
    if workspace_id.is_empty() {
        bail!("resolve label: workspace_id is required to resolve label id prefixes");
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("resolve label: label id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve label: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve label: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let workspace = form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>();
    let result: Value = client
        .get_json(&format!("/api/labels?workspace_id={workspace}"))
        .await
        .context("resolve label")?;
    let mut matches = result
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|label| (value_string(label, "id"), value_string(label, "name")))
        .filter(|(id, _)| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [(id, display)] => Ok((
            id.clone(),
            if display.is_empty() {
                id.clone()
            } else {
                display.clone()
            },
        )),
        [] => bail!(
            "no label found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous label id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

fn looks_like_issue_identifier(input: &str) -> bool {
    let Some((prefix, number)) = input.rsplit_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && number.trim().parse::<i64>().is_ok_and(|number| number > 0)
}

fn format_issue_get_table(issue: &Value, actors: &IssueActorNames) -> String {
    let id = value_string(issue, "id");
    let key = match value_string(issue, "identifier") {
        value if value.is_empty() => id,
        value => value,
    };
    let actor_type = value_string(issue, "assignee_type");
    let actor_id = value_string(issue, "assignee_id");
    let assignee = if actor_type.is_empty() || actor_id.is_empty() {
        String::new()
    } else {
        let actor_key = format!("{actor_type}:{actor_id}");
        actors
            .0
            .get(&actor_key)
            .map_or_else(|| actor_key.clone(), |name| format!("{actor_type}:{name}"))
    };
    let date = |field| {
        value_string(issue, field)
            .chars()
            .take(10)
            .collect::<String>()
    };
    format_table(&[
        vec![
            "KEY".into(),
            "TITLE".into(),
            "STATUS".into(),
            "PRIORITY".into(),
            "ASSIGNEE".into(),
            "START DATE".into(),
            "DUE DATE".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            key,
            value_string(issue, "title"),
            value_string(issue, "status"),
            value_string(issue, "priority"),
            assignee,
            date("start_date"),
            date("due_date"),
            value_string(issue, "description"),
        ],
    ])
}

async fn run_issue_pull_requests(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .get_json(&format!("/api/issues/{issue_id}/pull-requests"))
        .await
        .context("list issue pull requests")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => format_issue_pull_requests_table(&result),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn format_issue_pull_requests_table(result: &Value) -> String {
    let pull_requests = result
        .get("pull_requests")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(pull_requests.len() + 1);
    rows.push(vec![
        "NUMBER".into(),
        "STATE".into(),
        "TITLE".into(),
        "URL".into(),
    ]);
    rows.extend(pull_requests.iter().map(|pull_request| {
        let url = match value_string(pull_request, "url") {
            value if value.is_empty() => value_string(pull_request, "html_url"),
            value => value,
        };
        vec![
            value_string(pull_request, "number"),
            value_string(pull_request, "state"),
            value_string(pull_request, "title"),
            url,
        ]
    }));
    format_table(&rows)
}

#[derive(Debug, Serialize)]
struct AttachPullRequestBody {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sha: Option<String>,
}

async fn run_issue_pull_request_attach(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePullRequestAttachArgs,
) -> Result<RunOutput> {
    let url = args.url.trim();
    if url.is_empty() {
        bail!("--url is required (https://github.com/{{owner}}/{{repo}}/pull/{{number}})");
    }
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let optional = |value: &Option<String>| {
        value
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
    };
    let body = AttachPullRequestBody {
        url: url.into(),
        title: optional(&args.title),
        state: optional(&args.state),
        branch: optional(&args.branch),
        head_sha: optional(&args.head_sha),
    };
    let result: Value = client
        .post_json(&format!("/api/issues/{issue_id}/pull-requests"), &body)
        .await
        .context("attach pull request")?;
    let wrapped = serde_json::json!({
        "pull_request": result.get("pull_request").cloned().unwrap_or(Value::Null)
    });
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&wrapped)?),
        OutputFormat::Table => format_issue_pull_requests_table(&serde_json::json!({
            "pull_requests": [wrapped["pull_request"].clone()]
        })),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

#[derive(Debug, Serialize)]
struct IssueChildStageGroup {
    stage: i64,
    total: usize,
    done: usize,
    issues: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct IssueChildrenEnvelope {
    stages: Vec<IssueChildStageGroup>,
    total: usize,
    unstaged: Vec<Value>,
}

async fn run_issue_children(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
    _full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let response: Value = client
        .get_json(&format!("/api/issues/{issue_id}/children"))
        .await
        .context("list child issues")?;
    let mut children = response
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    children.sort_by_key(|child| child_stage(child).map_or((true, 0), |stage| (false, stage)));
    let stdout = match output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&group_issue_children(&children))?
        ),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors = load_issue_actor_names(&client, &workspace_id, &children).await;
            format_issue_children_table(&children, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn child_stage(issue: &Value) -> Option<i64> {
    let value = issue.get("stage")?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
}

fn terminal_child_issue(issue: &Value) -> bool {
    let category = match value_string(issue, "status_category") {
        value if value.is_empty() => value_string(issue, "status"),
        value => value,
    };
    matches!(category.as_str(), "done" | "cancelled")
}

fn group_issue_children(children: &[Value]) -> IssueChildrenEnvelope {
    let mut stages = Vec::<IssueChildStageGroup>::new();
    let mut index_by_stage = BTreeMap::<i64, usize>::new();
    let mut unstaged = Vec::new();
    for child in children {
        let Some(stage) = child_stage(child) else {
            unstaged.push(child.clone());
            continue;
        };
        let index = if let Some(index) = index_by_stage.get(&stage) {
            *index
        } else {
            stages.push(IssueChildStageGroup {
                stage,
                total: 0,
                done: 0,
                issues: Vec::new(),
            });
            let index = stages.len() - 1;
            index_by_stage.insert(stage, index);
            index
        };
        let group = &mut stages[index];
        group.total += 1;
        if terminal_child_issue(child) {
            group.done += 1;
        }
        group.issues.push(child.clone());
    }
    IssueChildrenEnvelope {
        stages,
        total: children.len(),
        unstaged,
    }
}

fn format_issue_children_table(children: &[Value], actors: &IssueActorNames) -> String {
    let mut rows = Vec::with_capacity(children.len() + 1);
    rows.push(vec![
        "STAGE".into(),
        "KEY".into(),
        "TITLE".into(),
        "STATUS".into(),
        "PRIORITY".into(),
        "ASSIGNEE".into(),
    ]);
    rows.extend(children.iter().map(|child| {
        let id = value_string(child, "id");
        let key = match value_string(child, "identifier") {
            value if value.is_empty() => id,
            value => value,
        };
        let actor_type = value_string(child, "assignee_type");
        let actor_id = value_string(child, "assignee_id");
        let assignee = if actor_type.is_empty() || actor_id.is_empty() {
            String::new()
        } else {
            let actor_key = format!("{actor_type}:{actor_id}");
            actors
                .0
                .get(&actor_key)
                .map_or_else(|| actor_key.clone(), |name| format!("{actor_type}:{name}"))
        };
        vec![
            child_stage(child).map_or_else(|| "-".into(), |stage| stage.to_string()),
            key,
            value_string(child, "title"),
            value_string(child, "status"),
            value_string(child, "priority"),
            assignee,
        ]
    }));
    format_table(&rows)
}

const BUILT_IN_ISSUE_STATUSES: &[&str] = &[
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "blocked",
    "cancelled",
];
const ISSUE_PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

#[derive(Debug)]
struct PendingAttachment {
    path: String,
    data: Vec<u8>,
}

async fn run_issue_create<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCreateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let title = args.title.as_deref().unwrap_or_default();
    if title.is_empty() {
        bail!("--title is required");
    }
    if let Some(status) = args.status.as_deref().filter(|value| !value.is_empty()) {
        validate_issue_status(status)?;
    }
    if let Some(priority) = args.priority.as_deref().filter(|value| !value.is_empty()) {
        validate_issue_priority(priority)?;
    }

    let mut client = new_api_client(cli, environment)?;
    if !args.attachment.is_empty() {
        let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT"))
            .max(std::time::Duration::from_secs(60));
        client = client.with_request_timeout(timeout);
    }

    let mut body = serde_json::Map::new();
    body.insert("title".into(), Value::String(title.into()));
    if let Some(description) = resolve_issue_create_description(args, environment, input)? {
        guard_issue_description_local_links(
            &description,
            environment,
            "Deliver the file itself with `cordy issue create --attachment <path>` (repeatable) and drop the link.",
        )?;
        body.insert("description".into(), Value::String(description));
    }
    if let Some(status) = args.status.as_deref().filter(|value| !value.is_empty()) {
        body.insert("status".into(), Value::String(status.into()));
    }
    if let Some(priority) = args.priority.as_deref().filter(|value| !value.is_empty()) {
        body.insert("priority".into(), Value::String(priority.into()));
    }
    if let Some(parent) = args.parent.as_deref().filter(|value| !value.is_empty()) {
        let parent_id = resolve_issue_ref(&client, parent)
            .await
            .context("resolve parent issue")?;
        body.insert("parent_issue_id".into(), Value::String(parent_id));
    }
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if let Some(project) = args.project.as_deref().filter(|value| !value.is_empty()) {
        let project_id = resolve_issue_project_id(&client, &workspace_id, project)
            .await
            .context("resolve project")?;
        body.insert("project_id".into(), Value::String(project_id));
    }
    if let Some(stage) = args.stage {
        if stage < 1 {
            bail!("--stage must be >= 1");
        }
        body.insert("stage".into(), Value::Number(stage.into()));
    }
    if let Some(start_date) = args.start_date.as_deref().filter(|value| !value.is_empty()) {
        body.insert("start_date".into(), Value::String(start_date.into()));
    }
    if let Some(due_date) = args.due_date.as_deref().filter(|value| !value.is_empty()) {
        body.insert("due_date".into(), Value::String(due_date.into()));
    }
    if args.allow_duplicate {
        body.insert("allow_duplicate".into(), Value::Bool(true));
    }
    if args.assignee.is_some() && args.assignee_id.is_some() {
        bail!("--assignee and --assignee-id are mutually exclusive");
    }
    let assignee = if let Some(id) = &args.assignee_id {
        Some(
            resolve_issue_assignee_id(&client, &workspace_id, id)
                .await
                .context("resolve assignee")?,
        )
    } else if let Some(name) = &args.assignee {
        Some(
            resolve_issue_assignee_name(&client, &workspace_id, name)
                .await
                .context("resolve assignee")?,
        )
    } else {
        None
    };
    if let Some(assignee) = assignee {
        body.insert("assignee_type".into(), Value::String(assignee.actor_type));
        body.insert("assignee_id".into(), Value::String(assignee.id));
    }
    if let Some(task_id) = environment
        .raw("CORDY_QUICK_CREATE_TASK_ID")
        .filter(|value| !value.is_empty())
    {
        body.insert("origin_type".into(), Value::String("quick_create".into()));
        body.insert("origin_id".into(), Value::String(task_id.into()));
    }
    let mut attachment_ids = append_unique_strings(args.attachment_id.iter().cloned());
    let env_attachment_ids = quick_create_attachment_ids(environment)?;
    attachment_ids = append_unique_strings(attachment_ids.into_iter().chain(env_attachment_ids));
    if !attachment_ids.is_empty() {
        body.insert(
            "attachment_ids".into(),
            Value::Array(attachment_ids.into_iter().map(Value::String).collect()),
        );
    }

    let (pending, mut stderr) =
        collect_local_attachments(&args.attachment, args.allow_external_file, environment)?;
    let issue: Value = match client.post_json("/api/issues", &body).await {
        Ok(issue) => issue,
        Err(error) => {
            if let Some(message) = active_duplicate_issue_message(&error) {
                bail!("{message}");
            }
            return Err(error).context("create issue");
        }
    };
    let issue_id = value_string(&issue, "id");
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => issue_id.clone(),
        value => value,
    };
    for attachment in pending {
        match client
            .upload_file(attachment.data, &attachment.path, &issue_id)
            .await
        {
            Ok(_) => {
                let _ = writeln!(stderr, "Uploaded {}", attachment.path);
            }
            Err(error) => {
                let _ = writeln!(
                    stderr,
                    "warning: upload attachment {} failed (issue already created, {}): {}",
                    attachment.path, issue_key, error
                );
            }
        }
    }
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => format_table(&[
            vec![
                "KEY".into(),
                "TITLE".into(),
                "STATUS".into(),
                "PRIORITY".into(),
            ],
            vec![
                issue_key,
                value_string(&issue, "title"),
                value_string(&issue, "status"),
                value_string(&issue, "priority"),
            ],
        ]),
    };
    Ok(RunOutput { stdout, stderr })
}

async fn run_issue_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueUpdateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    if let Some(status) = &args.status {
        validate_issue_status(status)?;
    }
    if let Some(priority) = &args.priority {
        validate_issue_priority(priority)?;
    }
    if args.assignee.is_some() && args.assignee_id.is_some() {
        bail!("--assignee and --assignee-id are mutually exclusive");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let mut body = serde_json::Map::new();
    if let Some(title) = &args.title {
        body.insert("title".into(), Value::String(title.clone()));
    }
    if args.description.is_some() || args.description_stdin || args.description_file.is_some() {
        let description = resolve_issue_update_description(args, environment, input)?;
        guard_issue_description_local_links(
            &description,
            environment,
            "`cordy issue update` cannot carry files — deliver the file with `cordy issue comment add <issue-id> --attachment <path>` instead, and drop the link.",
        )?;
        body.insert("description".into(), Value::String(description));
    }
    if let Some(status) = &args.status {
        body.insert("status".into(), Value::String(status.clone()));
    }
    if let Some(priority) = &args.priority {
        body.insert("priority".into(), Value::String(priority.clone()));
    }
    if let Some(project) = &args.project {
        if project.is_empty() {
            body.insert("project_id".into(), Value::Null);
        } else {
            let project_id = resolve_issue_project_id(&client, &workspace_id, project)
                .await
                .context("resolve project")?;
            body.insert("project_id".into(), Value::String(project_id));
        }
    }
    if let Some(start_date) = &args.start_date {
        body.insert("start_date".into(), Value::String(start_date.clone()));
    }
    if let Some(due_date) = &args.due_date {
        body.insert("due_date".into(), Value::String(due_date.clone()));
    }
    let assignee = if let Some(id) = &args.assignee_id {
        Some(
            resolve_issue_assignee_id(&client, &workspace_id, id)
                .await
                .context("resolve assignee")?,
        )
    } else if let Some(name) = &args.assignee {
        Some(
            resolve_issue_assignee_name(&client, &workspace_id, name)
                .await
                .context("resolve assignee")?,
        )
    } else {
        None
    };
    if let Some(assignee) = assignee {
        body.insert("assignee_type".into(), Value::String(assignee.actor_type));
        body.insert("assignee_id".into(), Value::String(assignee.id));
    }
    if let Some(parent) = &args.parent {
        if parent.is_empty() {
            body.insert("parent_issue_id".into(), Value::Null);
        } else {
            let parent_id = resolve_issue_ref(&client, parent)
                .await
                .context("resolve parent issue")?;
            body.insert("parent_issue_id".into(), Value::String(parent_id));
        }
    }
    if let Some(stage) = args.stage {
        if stage < 1 {
            bail!("--stage must be >= 1");
        }
        body.insert("stage".into(), Value::Number(stage.into()));
    }
    if let Some(position) = args.position {
        let position =
            serde_json::Number::from_f64(position).context("--position must be a finite number")?;
        body.insert("position".into(), Value::Number(position));
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --status, --priority, --assignee, etc."
        );
    }
    if args.no_start {
        body.insert("suppress_run".into(), Value::Bool(true));
    }

    let issue: Value = client
        .put_json(&format!("/api/issues/{issue_id}"), &body)
        .await
        .context("update issue")?;
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => value_string(&issue, "id"),
        value => value,
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => format_table(&[
            vec![
                "KEY".into(),
                "TITLE".into(),
                "STATUS".into(),
                "PRIORITY".into(),
            ],
            vec![
                issue_key,
                value_string(&issue, "title"),
                value_string(&issue, "status"),
                value_string(&issue, "priority"),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_issue_assign(
    cli: &Cli,
    environment: &Environment,
    args: &IssueAssignArgs,
) -> Result<RunOutput> {
    if args.to.is_none() && args.to_id.is_none() && !args.unassign {
        bail!("provide --to <name>, --to-id <uuid>, or --unassign");
    }
    if (args.to.is_some() || args.to_id.is_some()) && args.unassign {
        bail!("--to/--to-id and --unassign are mutually exclusive");
    }
    if args.to.is_some() && args.to_id.is_some() {
        bail!("--to and --to-id are mutually exclusive");
    }
    if args.no_start && args.unassign {
        bail!("--no-start cannot be used with --unassign");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let mut body = serde_json::Map::new();
    let display_target = if args.unassign {
        body.insert("assignee_type".into(), Value::Null);
        body.insert("assignee_id".into(), Value::Null);
        None
    } else {
        let workspace_id = resolve_current_workspace_id(cli, environment);
        let assignee = if let Some(id) = &args.to_id {
            resolve_issue_assignee_id(&client, &workspace_id, id)
                .await
                .context("resolve assignee")?
        } else {
            resolve_issue_assignee_name(
                &client,
                &workspace_id,
                args.to.as_deref().unwrap_or_default(),
            )
            .await
            .context("resolve assignee")?
        };
        let display = args.to.clone().unwrap_or_else(|| {
            if assignee.name.is_empty() {
                format!("{}:{}", assignee.actor_type, assignee.id)
            } else {
                format!("{}:{}", assignee.actor_type, assignee.name)
            }
        });
        body.insert("assignee_type".into(), Value::String(assignee.actor_type));
        body.insert("assignee_id".into(), Value::String(assignee.id));
        if args.no_start {
            body.insert("suppress_run".into(), Value::Bool(true));
        }
        Some(display)
    };

    let issue: Value = client
        .put_json(&format!("/api/issues/{issue_id}"), &body)
        .await
        .context("assign issue")?;
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => value_string(&issue, "id"),
        value => value,
    };
    let stderr = if let Some(target) = display_target {
        format!("Issue {issue_key} assigned to {target}.\n")
    } else {
        format!("Issue {issue_key} unassigned.\n")
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput { stdout, stderr })
}

async fn run_issue_status(
    cli: &Cli,
    environment: &Environment,
    args: &IssueStatusArgs,
) -> Result<RunOutput> {
    validate_issue_status(&args.status)?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let mut body =
        serde_json::Map::from_iter([("status".into(), Value::String(args.status.clone()))]);
    if args.no_start {
        body.insert("suppress_run".into(), Value::Bool(true));
    }
    let issue: Value = client
        .put_json(&format!("/api/issues/{issue_id}"), &body)
        .await
        .context("update status")?;
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => value_string(&issue, "id"),
        value => value,
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput {
        stdout,
        stderr: format!("Issue {issue_key} status changed to {}.\n", args.status),
    })
}

async fn run_issue_reorder(
    cli: &Cli,
    environment: &Environment,
    args: &IssueReorderArgs,
) -> Result<RunOutput> {
    if args.before.as_deref() == Some("") {
        bail!("--before requires an issue ID or key");
    }
    if args.after.as_deref() == Some("") {
        bail!("--after requires an issue ID or key");
    }
    if args.top == Some(false) {
        bail!("--top cannot be set to false; pass it on its own to move the issue to the top of its column");
    }
    if args.bottom == Some(false) {
        bail!("--bottom cannot be set to false; pass it on its own to move the issue to the bottom of its column");
    }

    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if workspace_id.is_empty() {
        bail!("no workspace configured; pass --workspace-id, set CORDY_WORKSPACE_ID, or configure a default workspace");
    }
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let target: Value = client
        .get_json(&format!("/api/issues/{issue_id}"))
        .await
        .context("get issue")?;
    let issue_key = issue_value_key(&target);
    let status = value_string(&target, "status");
    if status.is_empty() {
        bail!("issue {issue_key} has no status, cannot determine its column");
    }

    let relative_input = args.before.as_deref().or(args.after.as_deref());
    let other = if let Some(input) = relative_input {
        let id = resolve_issue_ref(&client, input)
            .await
            .context("resolve target issue")?;
        if id == issue_id {
            bail!("cannot reorder issue {issue_key} relative to itself");
        }
        Some((id, input.to_string()))
    } else {
        None
    };

    let project_id = value_string(&target, "project_id");
    let column = fetch_issue_column(&client, &workspace_id, &project_id, &status).await?;
    let mut positions = HashMap::with_capacity(column.len());
    let mut ordered = Vec::with_capacity(column.len());
    for issue in &column {
        let id = value_string(issue, "id");
        if id.is_empty() {
            continue;
        }
        positions.insert(
            id.clone(),
            issue.get("position").and_then(Value::as_f64).unwrap_or(0.0),
        );
        if id != issue_id {
            ordered.push(id);
        }
    }
    if ordered.is_empty() {
        if let Some((other_id, other_display)) = &other {
            return Err(reorder_target_not_in_column(
                &client,
                other_id,
                other_display,
                &issue_key,
                &status,
            )
            .await);
        }
        return issue_reorder_output(
            &target,
            args.output,
            format!(
                "Issue {issue_key} is the only issue in the {status} column; nothing to reorder.\n"
            ),
        );
    }

    let insert_index = if args.top == Some(true) {
        0
    } else if args.bottom == Some(true) {
        ordered.len()
    } else {
        let Some((other_id, other_display)) = other.as_ref() else {
            bail!("exactly one of --before, --after, --top, or --bottom is required");
        };
        let Some(index) = ordered.iter().position(|id| id == other_id) else {
            return Err(reorder_target_not_in_column(
                &client,
                other_id,
                other_display,
                &issue_key,
                &status,
            )
            .await);
        };
        index + usize::from(args.after.is_some())
    };
    let mut reordered = Vec::with_capacity(ordered.len() + 1);
    reordered.extend_from_slice(&ordered[..insert_index]);
    reordered.push(issue_id.clone());
    reordered.extend_from_slice(&ordered[insert_index..]);
    let current_position = positions.get(&issue_id).copied().unwrap_or(0.0);
    let new_position =
        compute_reorder_position(&reordered, &issue_id, &positions, current_position);
    if new_position == current_position {
        return issue_reorder_output(
            &target,
            args.output,
            format!("Issue {issue_key} is already in that position.\n"),
        );
    }

    let issue: Value = client
        .put_json(
            &format!("/api/issues/{issue_id}"),
            &serde_json::json!({"position": new_position}),
        )
        .await
        .context("reorder issue")?;
    let result_key = issue_value_key(&issue);
    issue_reorder_output(
        &issue,
        args.output,
        format!("Issue {result_key} reordered.\n"),
    )
}

fn issue_value_key(issue: &Value) -> String {
    match value_string(issue, "identifier") {
        value if value.is_empty() => value_string(issue, "id"),
        value => value,
    }
}

fn issue_reorder_output(issue: &Value, output: OutputFormat, stderr: String) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(issue)?),
        OutputFormat::Table => format_table(&[
            vec![
                "KEY".into(),
                "TITLE".into(),
                "STATUS".into(),
                "PRIORITY".into(),
            ],
            vec![
                issue_value_key(issue),
                value_string(issue, "title"),
                value_string(issue, "status"),
                value_string(issue, "priority"),
            ],
        ]),
    };
    Ok(RunOutput { stdout, stderr })
}

async fn fetch_issue_column(
    client: &ApiClient,
    workspace_id: &str,
    project_id: &str,
    status: &str,
) -> Result<Vec<Value>> {
    let mut issues = Vec::new();
    let mut offset = 0_i64;
    loop {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("workspace_id", workspace_id);
        serializer.append_pair("status", status);
        if !project_id.is_empty() {
            serializer.append_pair("project_id", project_id);
        }
        serializer.append_pair("sort", "position");
        serializer.append_pair("limit", "100");
        serializer.append_pair("offset", &offset.to_string());
        let result: Value = client
            .get_json(&format!("/api/issues?{}", serializer.finish()))
            .await
            .with_context(|| format!("list {status} column"))?;
        let page = result
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_len = page.len() as i64;
        issues.extend(page);
        offset += page_len;
        let total = result.get("total").and_then(Value::as_i64).unwrap_or(0);
        if page_len == 0 || offset >= total {
            break;
        }
    }
    Ok(issues)
}

async fn reorder_target_not_in_column(
    client: &ApiClient,
    other_id: &str,
    other_display: &str,
    issue_display: &str,
    status: &str,
) -> anyhow::Error {
    if let Ok(other) = client
        .get_json::<Value>(&format!("/api/issues/{other_id}"))
        .await
    {
        let other_status = value_string(&other, "status");
        if !other_status.is_empty() && other_status != status {
            return anyhow::anyhow!(
                "issue {other_display} is in the {other_status:?} column but {issue_display} is in {status:?}; move one with `cordy issue status` first, or pick a target in the same column"
            );
        }
    }
    anyhow::anyhow!("issue {other_display} was not found in the {status:?} column")
}

fn compute_reorder_position(
    ids: &[String],
    active_id: &str,
    positions: &HashMap<String, f64>,
    fallback: f64,
) -> f64 {
    let Some(index) = ids.iter().position(|id| id == active_id) else {
        return fallback;
    };
    if ids.len() == 1 {
        fallback
    } else if index == 0 {
        positions.get(&ids[1]).copied().unwrap_or(0.0) - 1.0
    } else if index == ids.len() - 1 {
        positions.get(&ids[index - 1]).copied().unwrap_or(0.0) + 1.0
    } else {
        (positions.get(&ids[index - 1]).copied().unwrap_or(0.0)
            + positions.get(&ids[index + 1]).copied().unwrap_or(0.0))
            / 2.0
    }
}

async fn run_issue_comment_add<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCommentAddArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let Some(content) = resolve_issue_comment_content(args, environment, input)? else {
        bail!("--content, --content-stdin, or --content-file is required");
    };
    guard_issue_description_local_links(
        &content,
        environment,
        "Deliver the file itself with `cordy issue comment add <issue-id> --attachment <path>` (repeatable) and drop the link.",
    )?;

    let mut client = new_api_client(cli, environment)?;
    if !args.attachment.is_empty() {
        let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT"))
            .max(std::time::Duration::from_secs(60));
        client = client.with_request_timeout(timeout);
    }
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let (pending, mut stderr) =
        collect_local_attachments(&args.attachment, args.allow_external_file, environment)?;
    let mut attachment_ids = Vec::with_capacity(pending.len());
    for attachment in pending {
        let id = client
            .upload_file(attachment.data, &attachment.path, &issue_id)
            .await
            .with_context(|| format!("upload attachment {}", attachment.path))?;
        attachment_ids.push(id);
        let _ = writeln!(stderr, "Uploaded {}", attachment.path);
    }

    let mut body = serde_json::Map::from_iter([("content".into(), Value::String(content))]);
    if let Some(parent_id) = args.parent.as_deref().filter(|value| !value.is_empty()) {
        body.insert("parent_id".into(), Value::String(parent_id.into()));
    }
    if !attachment_ids.is_empty() {
        body.insert(
            "attachment_ids".into(),
            Value::Array(attachment_ids.into_iter().map(Value::String).collect()),
        );
    }
    let comment: Value = client
        .post_json(&format!("/api/issues/{issue_id}/comments"), &body)
        .await
        .context("add comment")?;
    let _ = writeln!(stderr, "Comment added to issue {}.", args.issue_id);
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&comment)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput { stdout, stderr })
}

async fn run_issue_comment_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCommentListArgs,
) -> Result<RunOutput> {
    let since = args.since.as_deref().unwrap_or_default();
    let thread = args.thread.as_deref().unwrap_or_default();
    let before = args.before.as_deref().unwrap_or_default();
    let before_id = args.before_id.as_deref().unwrap_or_default();
    if args.recent.is_some_and(|value| value <= 0) {
        bail!("--recent must be a positive integer");
    }
    if args.tail.is_some_and(|value| value < 0) {
        bail!("--tail must be a non-negative integer (0 returns just the thread root)");
    }
    if !thread.is_empty() && args.recent.is_some() {
        bail!("--thread and --recent are mutually exclusive");
    }
    if args.roots_only && !thread.is_empty() {
        bail!("--roots-only and --thread are mutually exclusive");
    }
    if args.roots_only && args.recent.is_some() {
        bail!("--roots-only and --recent are mutually exclusive");
    }
    if args.roots_only && args.tail.is_some() {
        bail!("--roots-only and --tail are mutually exclusive");
    }
    if args.roots_only && !before.is_empty() {
        bail!("--roots-only does not support --before / --before-id");
    }
    if args.tail.is_some() && thread.is_empty() {
        bail!("--tail requires --thread (it is a thread-scoped limit)");
    }
    if before.is_empty() != before_id.is_empty() {
        bail!("--before and --before-id must be set together (composite cursor for stable pagination)");
    }
    if !before.is_empty() && args.recent.is_none() && !(args.tail.is_some() && !thread.is_empty()) {
        bail!("--before / --before-id require --recent (thread cursor) or --thread + --tail (reply cursor)");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if !since.is_empty() {
        serializer.append_pair("since", since);
    }
    if args.roots_only {
        serializer.append_pair("roots_only", "true");
    }
    if args.summary {
        serializer.append_pair("summary", "true");
    }
    let fold_eligible = !args.roots_only && since.is_empty() && args.tail.is_none();
    if fold_eligible && !args.full {
        serializer.append_pair("fold", "true");
    }
    if !thread.is_empty() {
        serializer.append_pair("thread", thread);
    }
    if let Some(tail) = args.tail {
        serializer.append_pair("tail", &tail.to_string());
    }
    if let Some(recent) = args.recent {
        serializer.append_pair("recent", &recent.to_string());
    }
    if !before.is_empty() {
        serializer.append_pair("before", before);
        serializer.append_pair("before_id", before_id);
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        format!("/api/issues/{issue_id}/comments")
    } else {
        format!("/api/issues/{issue_id}/comments?{query}")
    };
    let (mut comments, headers): (Vec<Value>, _) = client
        .get_json_with_headers(&path)
        .await
        .context("list comments")?;
    let mut stderr = String::new();
    let next_before = headers
        .get("X-Cordy-Next-Before")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let next_before_id = headers
        .get("X-Cordy-Next-Before-Id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !next_before.is_empty() && !next_before_id.is_empty() {
        let label = if !thread.is_empty() && args.tail.is_some() {
            "Next reply cursor"
        } else {
            "Next thread cursor"
        };
        let _ = writeln!(
            stderr,
            "{label}: --before {next_before} --before-id {next_before_id}"
        );
    }

    let stdout = match args.output {
        OutputFormat::Json => {
            if args.compact {
                compact_issue_comments(&mut comments);
            }
            format!("{}\n", serde_json::to_string_pretty(&comments)?)
        }
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors = load_comment_actor_names(&client, &workspace_id, &comments).await;
            format_issue_comments_table(&comments, &actors)
        }
    };
    Ok(RunOutput { stdout, stderr })
}

fn compact_issue_comments(comments: &mut [Value]) {
    for comment in comments {
        let Some(object) = comment.as_object_mut() else {
            continue;
        };
        object.remove("issue_id");
        object.remove("source_task_id");
        if object.get("updated_at") == object.get("created_at") {
            object.remove("updated_at");
        }
        object.retain(|_, value| match value {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            _ => true,
        });
    }
}

async fn load_comment_actor_names(
    client: &ApiClient,
    workspace_id: &str,
    comments: &[Value],
) -> IssueActorNames {
    let synthetic_issues = comments
        .iter()
        .map(|comment| {
            serde_json::json!({
                "assignee_type": comment.get("author_type").cloned().unwrap_or(Value::Null),
                "assignee_id": comment.get("author_id").cloned().unwrap_or(Value::Null)
            })
        })
        .collect::<Vec<_>>();
    load_issue_actor_names(client, workspace_id, &synthetic_issues).await
}

fn format_issue_comments_table(comments: &[Value], actors: &IssueActorNames) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "PARENT".into(),
        "AUTHOR".into(),
        "TYPE".into(),
        "CONTENT".into(),
        "CREATED".into(),
    ]];
    for comment in comments {
        let content = value_string(comment, "content");
        let content = if content.chars().count() > 80 {
            format!("{}...", content.chars().take(77).collect::<String>())
        } else {
            content
        };
        let created = value_string(comment, "created_at")
            .chars()
            .take(16)
            .collect::<String>();
        let parent = match value_string(comment, "parent_id") {
            value if value.is_empty() => "—".into(),
            value => value,
        };
        let actor_type = value_string(comment, "author_type");
        let actor_id = value_string(comment, "author_id");
        let author = if actor_type.is_empty() || actor_id.is_empty() {
            String::new()
        } else {
            let actor_key = format!("{actor_type}:{actor_id}");
            actors
                .0
                .get(&actor_key)
                .map_or(actor_key, |name| format!("{actor_type}:{name}"))
        };
        rows.push(vec![
            value_string(comment, "id"),
            parent,
            author,
            value_string(comment, "type"),
            content,
            created,
        ]);
    }
    format_table(&rows)
}

fn resolve_issue_comment_content<R: Read>(
    args: &IssueCommentAddArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let inline = args.content.as_deref().unwrap_or_default();
    let content_file = args
        .content_file
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(Path::new);
    let sources = [
        args.content_stdin,
        !inline.is_empty(),
        content_file.is_some(),
    ]
    .into_iter()
    .filter(|source| *source)
    .count();
    if sources > 1 {
        bail!("--content, --content-stdin, and --content-file are mutually exclusive");
    }
    if args.content_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --content-stdin")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --content-stdin is empty");
        }
        return Ok(Some(body));
    }
    if let Some(path) = content_file {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "content",
        )?;
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(read_path).context("read file for --content-file")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --content-file is empty");
        }
        return Ok(Some(body));
    }
    Ok((!inline.is_empty()).then(|| unescape_backslash_escapes(inline)))
}

async fn run_issue_comment_delete(
    cli: &Cli,
    environment: &Environment,
    comment_id: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!("/api/comments/{comment_id}"))
        .await
        .context("delete comment")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Comment {comment_id} deleted.\n"),
    })
}

async fn run_issue_comment_resolution(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCommentResolutionArgs,
    resolve: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let comment_id = args.comment_id.trim();
    let encoded_id = form_urlencoded::byte_serialize(comment_id.as_bytes()).collect::<String>();
    let path = format!("/api/comments/{encoded_id}/resolve");
    let comment: Value = if resolve {
        client
            .post_json(&path, &Value::Null)
            .await
            .context("resolve comment")?
    } else {
        client
            .delete_json(&path)
            .await
            .context("unresolve comment")?
    };
    let action = if resolve { "resolved" } else { "unresolved" };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&comment)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput {
        stdout,
        stderr: format!("Comment {comment_id} {action}.\n"),
    })
}

async fn run_issue_runs(
    cli: &Cli,
    environment: &Environment,
    args: &IssueRunsArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let runs: Vec<Value> = client
        .get_json(&format!("/api/issues/{issue_id}/task-runs"))
        .await
        .context("list runs")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&runs)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let synthetic = runs
                .iter()
                .map(|run| {
                    serde_json::json!({
                        "assignee_type":"agent",
                        "assignee_id":run.get("agent_id").cloned().unwrap_or(Value::Null)
                    })
                })
                .collect::<Vec<_>>();
            let actors = load_issue_actor_names(&client, &workspace_id, &synthetic).await;
            format_issue_runs_table(&runs, args.full_id, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn format_issue_runs_table(runs: &[Value], full_id: bool, actors: &IssueActorNames) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "AGENT".into(),
        "STATUS".into(),
        "STARTED".into(),
        "COMPLETED".into(),
        "ERROR".into(),
    ]];
    for run in runs {
        let agent_id = value_string(run, "agent_id");
        let agent = actors
            .0
            .get(&format!("agent:{agent_id}"))
            .cloned()
            .unwrap_or(agent_id);
        let error = value_string(run, "error");
        let error = if error.chars().count() > 50 {
            format!("{}...", error.chars().take(47).collect::<String>())
        } else {
            error
        };
        let timestamp = |field| {
            value_string(run, field)
                .chars()
                .take(16)
                .collect::<String>()
        };
        rows.push(vec![
            display_id(&value_string(run, "id"), full_id),
            agent,
            value_string(run, "status"),
            timestamp("started_at"),
            timestamp("completed_at"),
            error,
        ]);
    }
    format_table(&rows)
}

async fn resolve_task_run_scope(client: &ApiClient, issue: Option<&str>) -> Result<Option<String>> {
    match issue {
        Some(issue) if !issue.is_empty() => Ok(Some(
            resolve_issue_ref(client, issue)
                .await
                .context("resolve issue")?,
        )),
        _ => Ok(None),
    }
}

async fn run_issue_run_messages(
    cli: &Cli,
    environment: &Environment,
    args: &IssueRunMessagesArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_task_run_scope(&client, args.issue.as_deref()).await?;
    let task_id = resolve_task_run_id(&client, issue_id.as_deref(), &args.task_id)
        .await
        .context("resolve task run")?;
    let mut path = format!("/api/tasks/{task_id}/messages");
    if args.since > 0 {
        let _ = write!(path, "?since={}", args.since);
    }
    let messages: Vec<Value> = client.get_json(&path).await.context("list run messages")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&messages)?),
            OutputFormat::Table => format_issue_run_messages_table(&messages),
        },
        stderr: String::new(),
    })
}

fn format_issue_run_messages_table(messages: &[Value]) -> String {
    let mut rows = vec![vec![
        "SEQ".into(),
        "TYPE".into(),
        "TOOL".into(),
        "CONTENT".into(),
    ]];
    for message in messages {
        let mut content = value_string(message, "content");
        if content.is_empty() {
            content = value_string(message, "output");
        }
        if content.chars().count() > 80 {
            content = format!("{}...", content.chars().take(77).collect::<String>());
        }
        rows.push(vec![
            message
                .get("seq")
                .map(|value| format_metadata_value(Some(value)))
                .unwrap_or_default(),
            value_string(message, "type"),
            value_string(message, "tool"),
            content,
        ]);
    }
    format_table(&rows)
}

async fn run_issue_cancel_task(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCancelTaskArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_task_run_scope(&client, args.issue.as_deref()).await?;
    let task_id = resolve_task_run_id(&client, issue_id.as_deref(), &args.task_id)
        .await
        .context("resolve task run")?;
    let result: Value = client
        .post_json(
            &format!("/api/tasks/{task_id}/cancel"),
            &serde_json::Map::<String, Value>::new(),
        )
        .await
        .context("cancel task")?;
    let status = match value_string(&result, "status") {
        status if status.is_empty() => "cancelled".into(),
        status => status,
    };
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!("Task {task_id} -> status={status}\n"),
        },
        stderr: String::new(),
    })
}

async fn run_issue_usage(
    cli: &Cli,
    environment: &Environment,
    args: &IssueUsageArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let usage: Value = client
        .get_json(&format!("/api/issues/{issue_id}/usage"))
        .await
        .context("get issue usage")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&usage)?),
        OutputFormat::Table => format_table(&[
            vec![
                "INPUT_TOKENS".into(),
                "OUTPUT_TOKENS".into(),
                "CACHE_READ".into(),
                "CACHE_WRITE".into(),
                "RUNS".into(),
            ],
            vec![
                format_metadata_value(usage.get("total_input_tokens")),
                format_metadata_value(usage.get("total_output_tokens")),
                format_metadata_value(usage.get("total_cache_read_tokens")),
                format_metadata_value(usage.get("total_cache_write_tokens")),
                format_metadata_value(usage.get("task_count")),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn format_metadata_value(value: Option<&Value>) -> String {
    match value.unwrap_or(&Value::Null) {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                value.to_string()
            } else if let Some(value) = value.as_u64() {
                value.to_string()
            } else if let Some(value) = value.as_f64() {
                if value.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    value.to_string()
                }
            } else {
                value.to_string()
            }
        }
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

async fn run_issue_rerun(
    cli: &Cli,
    environment: &Environment,
    args: &IssueRerunArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let task: Value = client
        .post_json(
            &format!("/api/issues/{issue_id}/rerun"),
            &serde_json::Map::<String, Value>::new(),
        )
        .await
        .context("rerun issue")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&task)?),
        OutputFormat::Table => {
            let agent_id = value_string(&task, "agent_id");
            let synthetic = [serde_json::json!({
                "assignee_type":"agent","assignee_id":agent_id.clone()
            })];
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors = load_issue_actor_names(&client, &workspace_id, &synthetic).await;
            let agent = actors
                .0
                .get(&format!("agent:{agent_id}"))
                .cloned()
                .unwrap_or(agent_id);
            format!(
                "Re-enqueued task {} on agent {agent}\n",
                value_string(&task, "id")
            )
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_issue_search(
    cli: &Cli,
    environment: &Environment,
    args: &IssueSearchArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", &args.query);
    if args.limit > 0 {
        serializer.append_pair("limit", &args.limit.to_string());
    }
    if args.include_closed {
        serializer.append_pair("include_closed", "true");
    }
    let result: Value = client
        .get_json(&format!("/api/issues/search?{}", serializer.finish()))
        .await
        .context("search issues")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => {
            let issues = result
                .get("issues")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            format_issue_search_table(issues)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn format_issue_search_table(issues: &[Value]) -> String {
    let mut rows = vec![vec![
        "KEY".into(),
        "TITLE".into(),
        "STATUS".into(),
        "MATCH".into(),
    ]];
    for issue in issues {
        let mut match_info = value_string(issue, "match_source");
        let snippet = value_string(issue, "matched_snippet");
        if !snippet.is_empty() {
            let snippet = if snippet.chars().count() > 50 {
                format!("{}...", snippet.chars().take(47).collect::<String>())
            } else {
                snippet
            };
            match_info.push_str(": ");
            match_info.push_str(&snippet);
        }
        rows.push(vec![
            value_string(issue, "identifier"),
            value_string(issue, "title"),
            value_string(issue, "status"),
            match_info,
        ]);
    }
    format_table(&rows)
}

async fn run_issue_subscriber_list(
    cli: &Cli,
    environment: &Environment,
    issue_ref: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, issue_ref)
        .await
        .context("resolve issue")?;
    let subscribers: Vec<Value> = client
        .get_json(&format!("/api/issues/{issue_id}/subscribers"))
        .await
        .context("list subscribers")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&subscribers)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let synthetic = subscribers
                .iter()
                .map(|subscriber| {
                    serde_json::json!({
                        "assignee_type": subscriber.get("user_type").cloned().unwrap_or(Value::Null),
                        "assignee_id": subscriber.get("user_id").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect::<Vec<_>>();
            let actors = load_issue_actor_names(&client, &workspace_id, &synthetic).await;
            format_issue_subscribers_table(&subscribers, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn format_issue_subscribers_table(subscribers: &[Value], actors: &IssueActorNames) -> String {
    let mut rows = vec![vec!["USER".into(), "REASON".into(), "CREATED".into()]];
    for subscriber in subscribers {
        let actor_type = value_string(subscriber, "user_type");
        let actor_id = value_string(subscriber, "user_id");
        let actor_key = format!("{actor_type}:{actor_id}");
        let actor = actors
            .0
            .get(&actor_key)
            .map_or(actor_key, |name| format!("{actor_type}:{name}"));
        rows.push(vec![
            actor,
            value_string(subscriber, "reason"),
            value_string(subscriber, "created_at")
                .chars()
                .take(16)
                .collect(),
        ]);
    }
    format_table(&rows)
}

async fn run_issue_subscriber_mutation(
    cli: &Cli,
    environment: &Environment,
    args: &IssueSubscriberMutationArgs,
    subscribe: bool,
) -> Result<RunOutput> {
    if args.user.is_some() && args.user_id.is_some() {
        bail!("--user and --user-id are mutually exclusive");
    }
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let resolved = if let Some(user_id) = &args.user_id {
        Some(
            resolve_subscriber_id(&client, &workspace_id, user_id)
                .await
                .context("resolve user")?,
        )
    } else if let Some(user) = &args.user {
        Some(
            resolve_subscriber_name(&client, &workspace_id, user)
                .await
                .context("resolve user")?,
        )
    } else {
        None
    };
    let mut body = serde_json::Map::new();
    if let Some(actor) = &resolved {
        body.insert("user_type".into(), Value::String(actor.actor_type.clone()));
        body.insert("user_id".into(), Value::String(actor.id.clone()));
    }
    let action = if subscribe {
        "subscribe"
    } else {
        "unsubscribe"
    };
    let result: Value = client
        .post_json(&format!("/api/issues/{issue_id}/{action}"), &body)
        .await
        .with_context(|| format!("{action} issue"))?;
    let target = if let Some(user) = args.user.as_deref() {
        user.into()
    } else if let Some(actor) = resolved {
        if actor.name.is_empty() {
            format!("{}:{}", actor.actor_type, actor.id)
        } else {
            format!("{}:{}", actor.actor_type, actor.name)
        }
    } else {
        "caller".into()
    };
    let verb = if subscribe {
        "Subscribed"
    } else {
        "Unsubscribed"
    };
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!("{verb} {target} to issue {}.\n", args.issue_id),
    })
}

fn issue_labels(result: &Value) -> &[Value] {
    result
        .get("labels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn format_issue_labels(labels: &[Value], output: OutputFormat, full_id: bool) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(labels)?)),
        OutputFormat::Table => Ok(format_label_table(labels, full_id)),
    }
}

fn format_label_table(labels: &[Value], full_id: bool) -> String {
    let mut rows = vec![vec!["ID".into(), "NAME".into(), "COLOR".into()]];
    rows.extend(labels.iter().map(|label| {
        vec![
            display_id(&value_string(label, "id"), full_id),
            value_string(label, "name"),
            value_string(label, "color"),
        ]
    }));
    format_table(&rows)
}

fn format_workspace_label_table(labels: &[Value], full_id: bool) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "COLOR".into(),
        "CREATED".into(),
    ]];
    rows.extend(labels.iter().map(|label| {
        vec![
            display_id(&value_string(label, "id"), full_id),
            value_string(label, "name"),
            value_string(label, "color"),
            value_string(label, "created_at").chars().take(10).collect(),
        ]
    }));
    format_table(&rows)
}

fn format_label_result(
    label: &Value,
    output: OutputFormat,
    include_created: bool,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(label)?)),
        OutputFormat::Table if include_created => Ok(format_workspace_label_table(
            std::slice::from_ref(label),
            true,
        )),
        OutputFormat::Table => Ok(format_label_table(std::slice::from_ref(label), true)),
    }
}

async fn run_label_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let path = if workspace_id.is_empty() {
        "/api/labels".into()
    } else {
        format!(
            "/api/labels?workspace_id={}",
            form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
        )
    };
    let result: Value = client.get_json(&path).await.context("list labels")?;
    let labels = issue_labels(&result);
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(labels)?),
            OutputFormat::Table => format_workspace_label_table(labels, full_id),
        },
        stderr: String::new(),
    })
}

async fn run_label_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let label_id = resolve_label_id(&client, &workspace_id, id)
        .await
        .context("resolve label")?;
    let label: Value = client
        .get_json(&format!("/api/labels/{label_id}"))
        .await
        .context("get label")?;
    Ok(RunOutput {
        stdout: format_label_result(&label, output, true)?,
        stderr: String::new(),
    })
}

async fn run_label_create(
    cli: &Cli,
    environment: &Environment,
    args: &LabelCreateArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let color = args
        .color
        .as_deref()
        .filter(|color| !color.is_empty())
        .context("--color is required (e.g. #3b82f6)")?;
    let client = new_api_client(cli, environment)?;
    let label: Value = client
        .post_json(
            "/api/labels",
            &serde_json::json!({"name":name,"color":color}),
        )
        .await
        .context("create label")?;
    Ok(RunOutput {
        stdout: format_label_result(&label, args.output, false)?,
        stderr: String::new(),
    })
}

async fn run_label_update(
    cli: &Cli,
    environment: &Environment,
    args: &LabelUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let label_id = resolve_label_id(&client, &workspace_id, &args.id)
        .await
        .context("resolve label")?;
    let mut body = serde_json::Map::new();
    if let Some(name) = args.name.as_deref().filter(|name| !name.is_empty()) {
        body.insert("name".into(), Value::String(name.into()));
    }
    if let Some(color) = args.color.as_deref().filter(|color| !color.is_empty()) {
        body.insert("color".into(), Value::String(color.into()));
    }
    if body.is_empty() {
        bail!("nothing to update — provide --name and/or --color");
    }
    let label: Value = client
        .put_json(&format!("/api/labels/{label_id}"), &body)
        .await
        .context("update label")?;
    Ok(RunOutput {
        stdout: format_label_result(&label, args.output, false)?,
        stderr: String::new(),
    })
}

async fn run_label_delete(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (label_id, display) = resolve_label_reference(&client, &workspace_id, id)
        .await
        .context("resolve label")?;
    client
        .delete(&format!("/api/labels/{label_id}"))
        .await
        .context("delete label")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!(
                "{}\n",
                serde_json::to_string_pretty(&serde_json::json!({"id":label_id,"deleted":true}))?
            ),
            OutputFormat::Table => format!("Label {display} deleted.\n"),
        },
        stderr: String::new(),
    })
}

async fn run_issue_label_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .get_json(&format!("/api/issues/{issue_id}/labels"))
        .await
        .context("list issue labels")?;
    Ok(RunOutput {
        stdout: format_issue_labels(issue_labels(&result), args.output, args.full_id)?,
        stderr: String::new(),
    })
}

async fn resolve_issue_and_label(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelMutationArgs,
) -> Result<(ApiClient, String, String)> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let label_id = resolve_label_id(&client, &workspace_id, &args.label_id)
        .await
        .context("resolve label")?;
    Ok((client, issue_id, label_id))
}

async fn run_issue_label_add(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelMutationArgs,
) -> Result<RunOutput> {
    let (client, issue_id, label_id) = resolve_issue_and_label(cli, environment, args).await?;
    let result: Value = client
        .post_json(
            &format!("/api/issues/{issue_id}/labels"),
            &serde_json::json!({"label_id":label_id}),
        )
        .await
        .context("attach label")?;
    Ok(RunOutput {
        stdout: format_issue_labels(issue_labels(&result), args.output, args.full_id)?,
        stderr: String::new(),
    })
}

async fn run_issue_label_remove(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelMutationArgs,
) -> Result<RunOutput> {
    let (client, issue_id, label_id) = resolve_issue_and_label(cli, environment, args).await?;
    client
        .delete(&format!("/api/issues/{issue_id}/labels/{label_id}"))
        .await
        .context("detach label")?;
    let result = client
        .get_json::<Value>(&format!("/api/issues/{issue_id}/labels"))
        .await;
    let stdout = match result {
        Ok(result) => format_issue_labels(issue_labels(&result), args.output, args.full_id)?,
        Err(_) if args.output == OutputFormat::Json => "{\n  \"detached\": true\n}\n".into(),
        Err(_) => "Label detached.\n".into(),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn metadata_object(result: &Value) -> serde_json::Map<String, Value> {
    result
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn metadata_value_type(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        _ => "unknown",
    }
}

fn format_metadata_table(metadata: &serde_json::Map<String, Value>) -> String {
    let mut keys = metadata.keys().collect::<Vec<_>>();
    keys.sort();
    let mut rows = vec![vec!["KEY".into(), "VALUE".into(), "TYPE".into()]];
    rows.extend(keys.into_iter().map(|key| {
        let value = &metadata[key];
        vec![
            key.clone(),
            format_metadata_value(Some(value)),
            metadata_value_type(value).into(),
        ]
    }));
    format_table(&rows)
}

fn format_metadata_output(
    metadata: &serde_json::Map<String, Value>,
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(metadata)?)),
        OutputFormat::Table => Ok(format_metadata_table(metadata)),
    }
}

fn parse_metadata_value(raw: &str, forced_type: Option<&str>) -> Result<Value> {
    match forced_type.unwrap_or_default() {
        "string" => Ok(Value::String(raw.into())),
        "number" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Number(_)) => Ok(value),
            _ => bail!("value {raw:?} is not a valid number"),
        },
        "bool" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => bail!("value {raw:?} is not a valid bool (expected true or false)"),
        },
        "" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => Ok(value),
            _ => Ok(Value::String(raw.into())),
        },
        value_type => {
            bail!("unknown --type {value_type:?} (expected string, number, or bool)")
        }
    }
}

async fn run_issue_metadata_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result = client
        .get_json::<Value>(&format!("/api/issues/{issue_id}/metadata"))
        .await;
    let metadata = match result {
        Ok(result) => metadata_object(&result),
        Err(error)
            if error
                .downcast_ref::<HttpError>()
                .is_some_and(|error| error.status_code == 404) =>
        {
            serde_json::Map::new()
        }
        Err(error) => return Err(error).context("list metadata"),
    };
    Ok(RunOutput {
        stdout: format_metadata_output(&metadata, args.output)?,
        stderr: String::new(),
    })
}

async fn run_issue_metadata_get(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataKeyArgs,
) -> Result<RunOutput> {
    let key = args
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .context("--key is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .get_json(&format!("/api/issues/{issue_id}/metadata"))
        .await
        .context("get metadata")?;
    let metadata = metadata_object(&result);
    let value = metadata
        .get(key)
        .with_context(|| format!("key {key:?} not found on issue"))?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(value)?),
        OutputFormat::Table => format_table(&[
            vec!["KEY".into(), "VALUE".into(), "TYPE".into()],
            vec![
                key.into(),
                format_metadata_value(Some(value)),
                metadata_value_type(value).into(),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_issue_metadata_set(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataSetArgs,
) -> Result<RunOutput> {
    let key = args
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .context("--key is required")?;
    let raw = args.value.as_deref().context("--value is required")?;
    let value = parse_metadata_value(raw, args.value_type.as_deref())?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .put_json(
            &format!("/api/issues/{issue_id}/metadata/{key}"),
            &serde_json::json!({"value":value}),
        )
        .await
        .context("set metadata")?;
    let metadata = metadata_object(&result);
    Ok(RunOutput {
        stdout: format_metadata_output(&metadata, args.output)?,
        stderr: String::new(),
    })
}

async fn run_issue_metadata_delete(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataDeleteArgs,
) -> Result<RunOutput> {
    let key = args
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .context("--key is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    client
        .delete(&format!("/api/issues/{issue_id}/metadata/{key}"))
        .await
        .context("delete metadata")?;
    let result = client
        .get_json::<Value>(&format!("/api/issues/{issue_id}/metadata"))
        .await;
    let stdout = match result {
        Ok(result) => format_metadata_output(&metadata_object(&result), args.output)?,
        Err(_) if args.output == OutputFormat::Json => "{\n  \"deleted\": true\n}\n".into(),
        Err(_) => "Key deleted.\n".into(),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

#[derive(Debug)]
struct TimelineFilter {
    activity_only: bool,
    actions: HashSet<String>,
    since: Option<DateTime<FixedOffset>>,
    tail: usize,
}

fn build_timeline_filter(args: &IssueTimelineArgs) -> Result<TimelineFilter> {
    if args.tail < 0 {
        bail!("--tail must be >= 0");
    }
    let actions = args
        .action
        .iter()
        .map(|action| action.trim())
        .filter(|action| !action.is_empty())
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    let since = args
        .since
        .as_deref()
        .filter(|since| !since.is_empty())
        .map(|since| {
            DateTime::parse_from_rfc3339(since).with_context(|| {
                format!("invalid --since {since:?}: expected RFC3339, e.g. 2026-08-19T00:00:00Z")
            })
        })
        .transpose()?;
    Ok(TimelineFilter {
        activity_only: args.activity_only || !actions.is_empty(),
        actions,
        since,
        tail: args.tail as usize,
    })
}

fn filter_timeline(entries: Vec<Value>, filter: &TimelineFilter) -> Vec<Value> {
    let mut entries = entries
        .into_iter()
        .filter(|entry| {
            if filter.activity_only && value_string(entry, "type") != "activity" {
                return false;
            }
            if !filter.actions.is_empty()
                && !filter.actions.contains(&value_string(entry, "action"))
            {
                return false;
            }
            let Some(since) = filter.since.as_ref() else {
                return true;
            };
            DateTime::parse_from_rfc3339(&value_string(entry, "created_at"))
                .is_ok_and(|created| created > *since)
        })
        .collect::<Vec<_>>();
    if filter.tail > 0 && entries.len() > filter.tail {
        entries.drain(..entries.len() - filter.tail);
    }
    entries
}

fn timeline_actor_inputs(entries: &[Value]) -> Vec<Value> {
    let mut actors = Vec::new();
    for entry in entries {
        actors.push(serde_json::json!({
            "assignee_type":entry.get("actor_type").cloned().unwrap_or(Value::Null),
            "assignee_id":entry.get("actor_id").cloned().unwrap_or(Value::Null),
        }));
        if let Some(details) = entry.get("details") {
            for prefix in ["from", "to"] {
                actors.push(serde_json::json!({
                    "assignee_type":details.get(&format!("{prefix}_type")).cloned().unwrap_or(Value::Null),
                    "assignee_id":details.get(&format!("{prefix}_id")).cloned().unwrap_or(Value::Null),
                }));
            }
        }
    }
    actors
}

async fn run_issue_timeline(
    cli: &Cli,
    environment: &Environment,
    args: &IssueTimelineArgs,
) -> Result<RunOutput> {
    let filter = build_timeline_filter(args)?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let (entries, headers) = client
        .get_json_with_headers::<Vec<Value>>(&format!("/api/issues/{issue_id}/timeline"))
        .await
        .context("list issue timeline")?;
    let entries = filter_timeline(entries, &filter);
    let truncated = headers
        .get("X-Timeline-Truncated")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let stderr = if truncated.is_empty() {
        String::new()
    } else {
        format!(
            "warning: timeline truncated by the server cap ({truncated}): older entries are missing. Durations and \"first entered <status>\" cannot be concluded from this read.\n"
        )
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&entries)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actor_inputs = timeline_actor_inputs(&entries);
            let actors = load_issue_actor_names(&client, &workspace_id, &actor_inputs).await;
            format_issue_timeline_table(&entries, &actors, args.full_id)
        }
    };
    Ok(RunOutput { stdout, stderr })
}

fn timeline_actor(
    actor_type: &str,
    actor_id: &str,
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    match (actor_type.is_empty(), actor_id.is_empty()) {
        (true, true) => String::new(),
        (false, true) => actor_type.into(),
        (true, false) => display_id(actor_id, full_id),
        (false, false) => actors
            .0
            .get(&format!("{actor_type}:{actor_id}"))
            .map_or_else(
                || format!("{actor_type}:{}", display_id(actor_id, full_id)),
                |name| format!("{actor_type}:{name}"),
            ),
    }
}

fn timeline_transition(from: String, to: String) -> String {
    format!(
        "{} → {}",
        if from.is_empty() { "(none)" } else { &from },
        if to.is_empty() { "(none)" } else { &to }
    )
}

fn timeline_detail(entry: &Value, actors: &IssueActorNames, full_id: bool) -> String {
    if value_string(entry, "type") == "comment" {
        let content = value_string(entry, "content")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        return truncate_text(&content, 60);
    }
    let Some(details) = entry.get("details").and_then(Value::as_object) else {
        return String::new();
    };
    if details.contains_key("from") || details.contains_key("to") {
        return timeline_transition(
            value_string(&Value::Object(details.clone()), "from"),
            value_string(&Value::Object(details.clone()), "to"),
        );
    }
    if ["from_type", "from_id", "to_type", "to_id"]
        .iter()
        .any(|key| details.contains_key(*key))
    {
        let details = Value::Object(details.clone());
        return timeline_transition(
            timeline_actor(
                &value_string(&details, "from_type"),
                &value_string(&details, "from_id"),
                actors,
                full_id,
            ),
            timeline_actor(
                &value_string(&details, "to_type"),
                &value_string(&details, "to_id"),
                actors,
                full_id,
            ),
        );
    }
    let mut keys = details.keys().collect::<Vec<_>>();
    keys.sort();
    let text = keys
        .into_iter()
        .map(|key| {
            format!(
                "{key}={}",
                value_string(&Value::Object(details.clone()), key)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    truncate_text(&text, 60)
}

fn format_issue_timeline_table(
    entries: &[Value],
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    let mut rows = vec![vec![
        "TIME".into(),
        "TYPE".into(),
        "ACTOR".into(),
        "DETAIL".into(),
    ]];
    rows.extend(entries.iter().map(|entry| {
        let action = value_string(entry, "action");
        vec![
            value_string(entry, "created_at").chars().take(16).collect(),
            if action.is_empty() {
                value_string(entry, "type")
            } else {
                action
            },
            timeline_actor(
                &value_string(entry, "actor_type"),
                &value_string(entry, "actor_id"),
                actors,
                full_id,
            ),
            timeline_detail(entry, actors, full_id),
        ]
    }));
    format_table(&rows)
}

#[derive(Clone, Debug, Deserialize)]
struct PropertyOption {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PropertyConfig {
    #[serde(default)]
    options: Vec<PropertyOption>,
}

#[derive(Clone, Debug, Deserialize)]
struct PropertyDefinition {
    id: String,
    name: String,
    #[serde(rename = "type")]
    property_type: String,
    #[serde(default)]
    config: PropertyConfig,
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Serialize)]
struct IssuePropertyRow {
    property_id: String,
    name: String,
    #[serde(rename = "type")]
    property_type: String,
    value: Value,
    display: String,
    #[serde(skip_serializing_if = "is_false")]
    archived: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

async fn fetch_property_definitions(client: &ApiClient) -> Result<Vec<PropertyDefinition>> {
    let result: Value = client
        .get_json("/api/properties?include_archived=true")
        .await
        .context("list properties")?;
    serde_json::from_value(
        result
            .get("properties")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .context("decode properties")
}

fn resolve_property<'a>(
    properties: &'a [PropertyDefinition],
    reference: &str,
) -> Result<&'a PropertyDefinition> {
    if let Some(property) = properties.iter().find(|property| property.id == reference) {
        return Ok(property);
    }
    let reference = reference.trim();
    if let Some(property) = properties
        .iter()
        .find(|property| property.name.eq_ignore_ascii_case(reference))
    {
        return Ok(property);
    }
    bail!(
        "property {reference:?} not found; available: {}",
        properties
            .iter()
            .map(|property| property.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

async fn resolve_property_member(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<String> {
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required to resolve assignees; use --workspace-id or set CORDY_WORKSPACE_ID"
        );
    }
    let token = raw.trim();
    if let Some(id) = token.strip_prefix("member:") {
        let id = id.trim();
        if !is_canonical_uuid(id) {
            bail!("actor id in {token:?} must be a UUID");
        }
        return Ok(format!("member:{id}"));
    }
    let input = normalize_assignee_input(token);
    if input.is_empty() {
        bail!("actor value cannot be empty");
    }
    let members =
        retry_actor_get::<Vec<Value>>(client, &format!("/api/workspaces/{workspace_id}/members"))
            .await
            .context("fetch members")?;
    let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
    for member in &members {
        let id = value_string(member, "user_id");
        let name = value_string(member, "name");
        let email = value_string(member, "email");
        if id.eq_ignore_ascii_case(&input)
            || display_id(&id, false).eq_ignore_ascii_case(&input)
            || (!email.is_empty() && email.eq_ignore_ascii_case(&input))
        {
            buckets[0].push((id, name));
        } else if name.eq_ignore_ascii_case(&input) {
            buckets[1].push((id, name));
        } else if name
            .to_ascii_lowercase()
            .contains(&input.to_ascii_lowercase())
        {
            buckets[2].push((id, name));
        }
    }
    for bucket in buckets {
        match bucket.as_slice() {
            [] => {}
            [(id, _)] => return Ok(format!("member:{id}")),
            matches => {
                let matches = matches
                    .iter()
                    .map(|(id, name)| format!("  member {name:?} ({})", display_id(id, false)))
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!("ambiguous assignee {input:?}; matches:\n{matches}");
            }
        }
    }
    bail!("no member found matching {input:?}")
}

async fn encode_issue_property_value(
    client: &ApiClient,
    workspace_id: &str,
    property: &PropertyDefinition,
    raw: &str,
) -> Result<Value> {
    let valid_options = property
        .config
        .options
        .iter()
        .map(|option| option.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let resolve_option = |reference: &str| -> Result<String> {
        let reference = reference.trim();
        property
            .config
            .options
            .iter()
            .find(|option| option.id == reference || option.name.eq_ignore_ascii_case(reference))
            .map(|option| option.id.clone())
            .with_context(|| {
                format!(
                    "option {reference:?} not found on property {:?}; valid options: {valid_options}",
                    property.name
                )
            })
    };
    match property.property_type.as_str() {
        "select" => Ok(Value::String(resolve_option(raw)?)),
        "multi_select" => {
            let values = raw
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(resolve_option)
                .collect::<Result<Vec<_>>>()?;
            if values.is_empty() {
                bail!("--value must list at least one option; valid options: {valid_options}");
            }
            Ok(Value::Array(
                values.into_iter().map(Value::String).collect(),
            ))
        }
        "actor" => Ok(Value::String(
            resolve_property_member(client, workspace_id, raw).await?,
        )),
        "multi_actor" => {
            let mut values = Vec::new();
            for token in raw
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
            {
                values.push(Value::String(
                    resolve_property_member(client, workspace_id, token).await?,
                ));
            }
            if values.is_empty() {
                bail!("--value must list at least one member");
            }
            Ok(Value::Array(values))
        }
        "number" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Number(_)) => Ok(value),
            _ => bail!("value {raw:?} is not a valid number"),
        },
        "checkbox" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => bail!("value {raw:?} is not a valid bool (expected true or false)"),
        },
        _ => Ok(Value::String(raw.into())),
    }
}

fn actor_property_inputs(
    properties: &[PropertyDefinition],
    bag: &serde_json::Map<String, Value>,
) -> Vec<Value> {
    let mut inputs = Vec::new();
    for property in properties {
        if !matches!(property.property_type.as_str(), "actor" | "multi_actor") {
            continue;
        }
        let Some(value) = bag.get(&property.id) else {
            continue;
        };
        let values = value
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(value));
        for value in values {
            let Some(reference) = value.as_str() else {
                continue;
            };
            let Some((actor_type, actor_id)) = reference.split_once(':') else {
                continue;
            };
            inputs.push(serde_json::json!({"assignee_type":actor_type,"assignee_id":actor_id}));
        }
    }
    inputs
}

fn format_issue_property_value(
    property: &PropertyDefinition,
    value: &Value,
    actors: &IssueActorNames,
) -> String {
    let option_name = |id: &str| {
        property
            .config
            .options
            .iter()
            .find(|option| option.id == id)
            .map_or_else(|| id.into(), |option| option.name.clone())
    };
    let actor_name = |reference: &str| {
        actors
            .0
            .get(reference)
            .cloned()
            .unwrap_or_else(|| reference.into())
    };
    match property.property_type.as_str() {
        "select" => value.as_str().map(option_name),
        "multi_select" => value.as_array().map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(option_name)
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "actor" => value.as_str().map(actor_name),
        "multi_actor" => value.as_array().map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(actor_name)
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "checkbox" => value
            .as_bool()
            .map(|checked| if checked { "✓".into() } else { "✗".into() }),
        _ => None,
    }
    .unwrap_or_else(|| format_metadata_value(Some(value)))
}

fn build_issue_property_rows(
    properties: &[PropertyDefinition],
    bag: &serde_json::Map<String, Value>,
    actors: &IssueActorNames,
) -> Vec<IssuePropertyRow> {
    properties
        .iter()
        .filter_map(|property| {
            let value = bag.get(&property.id)?;
            Some(IssuePropertyRow {
                property_id: property.id.clone(),
                name: property.name.clone(),
                property_type: property.property_type.clone(),
                value: value.clone(),
                display: format_issue_property_value(property, value, actors),
                archived: property.archived,
            })
        })
        .collect()
}

fn format_issue_property_rows(rows: &[IssuePropertyRow], output: OutputFormat) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(rows)?)),
        OutputFormat::Table => {
            let mut table = vec![vec!["NAME".into(), "VALUE".into(), "TYPE".into()]];
            table.extend(rows.iter().map(|row| {
                vec![
                    row.name.clone(),
                    row.display.clone(),
                    row.property_type.clone(),
                ]
            }));
            Ok(format_table(&table))
        }
    }
}

async fn property_rows(
    client: &ApiClient,
    workspace_id: &str,
    properties: &[PropertyDefinition],
    bag: &serde_json::Map<String, Value>,
) -> Vec<IssuePropertyRow> {
    let inputs = actor_property_inputs(properties, bag);
    let actors = load_issue_actor_names(client, workspace_id, &inputs).await;
    build_issue_property_rows(properties, bag, &actors)
}

async fn run_issue_property_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePropertyListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let properties = fetch_property_definitions(&client).await?;
    let issue: Value = client
        .get_json(&format!("/api/issues/{issue_id}"))
        .await
        .context("get issue")?;
    let bag = issue
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let rows = property_rows(&client, &workspace_id, &properties, &bag).await;
    Ok(RunOutput {
        stdout: format_issue_property_rows(&rows, args.output)?,
        stderr: String::new(),
    })
}

async fn run_issue_property_set(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePropertyMutationArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let raw = args.value.as_deref().context("--value is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, name)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let value = encode_issue_property_value(&client, &workspace_id, property, raw).await?;
    let result: Value = client
        .put_json(
            &format!("/api/issues/{issue_id}/properties/{}", property.id),
            &serde_json::json!({"value":value}),
        )
        .await
        .context("set property")?;
    let bag = result
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let rows = property_rows(&client, &workspace_id, &properties, &bag).await;
    Ok(RunOutput {
        stdout: format_issue_property_rows(&rows, args.output)?,
        stderr: String::new(),
    })
}

async fn run_issue_property_unset(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePropertyUnsetArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, name)?;
    client
        .delete(&format!(
            "/api/issues/{issue_id}/properties/{}",
            property.id
        ))
        .await
        .context("unset property")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => "{\n  \"deleted\": true\n}\n".into(),
            OutputFormat::Table => format!("Property {:?} unset.\n", property.name),
        },
        stderr: String::new(),
    })
}

fn validate_issue_status(status: &str) -> Result<()> {
    let normalized = status.trim().to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    let valid = (1..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_');
    if !valid {
        if normalized.is_empty() {
            bail!(
                "invalid status {status:?}; valid values: {}",
                BUILT_IN_ISSUE_STATUSES.join(", ")
            );
        }
        bail!(
            "invalid status {status:?}; a status key is 1-32 characters of lowercase letters, digits or underscore. Built-in values: {}",
            BUILT_IN_ISSUE_STATUSES.join(", ")
        );
    }
    Ok(())
}

fn validate_issue_priority(priority: &str) -> Result<()> {
    if !ISSUE_PRIORITIES.contains(&priority) {
        bail!(
            "invalid priority {priority:?}; valid values: {}",
            ISSUE_PRIORITIES.join(", ")
        );
    }
    Ok(())
}

fn resolve_issue_create_description<R: Read>(
    args: &IssueCreateArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let inline = args.description.as_deref().unwrap_or_default();
    let description_file = args
        .description_file
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(Path::new);
    let sources = [
        args.description_stdin,
        !inline.is_empty(),
        description_file.is_some(),
    ]
    .into_iter()
    .filter(|source| *source)
    .count();
    if sources > 1 {
        bail!("--description, --description-stdin, and --description-file are mutually exclusive");
    }
    if args.description_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --description-stdin")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --description-stdin is empty");
        }
        return Ok(Some(body));
    }
    if let Some(path) = description_file {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "description",
        )?;
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(path).context("read file for --description-file")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --description-file is empty");
        }
        return Ok(Some(body));
    }
    Ok((!inline.is_empty()).then(|| unescape_backslash_escapes(inline)))
}

fn resolve_issue_update_description<R: Read>(
    args: &IssueUpdateArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<String> {
    let inline = args.description.as_deref().unwrap_or_default();
    let description_file = args
        .description_file
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(Path::new);
    let sources = [
        args.description_stdin,
        args.description
            .as_ref()
            .is_some_and(|_| !inline.is_empty()),
        description_file.is_some(),
    ]
    .into_iter()
    .filter(|source| *source)
    .count();
    if sources > 1 {
        bail!("--description, --description-stdin, and --description-file are mutually exclusive");
    }
    if args.description_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --description-stdin")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --description-stdin is empty");
        }
        return Ok(body);
    }
    if let Some(path) = description_file {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "description",
        )?;
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(path).context("read file for --description-file")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --description-file is empty");
        }
        return Ok(body);
    }
    Ok(unescape_backslash_escapes(inline))
}

fn append_unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && seen.insert(value.to_string()) {
            output.push(value.into());
        }
    }
    output
}

fn quick_create_attachment_ids(environment: &Environment) -> Result<Vec<String>> {
    let Some(raw) = environment
        .raw("CORDY_QUICK_CREATE_ATTACHMENT_IDS")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> =
        serde_json::from_str(raw).context("parse CORDY_QUICK_CREATE_ATTACHMENT_IDS")?;
    Ok(append_unique_strings(ids))
}

fn collect_local_attachments(
    attachments: &[String],
    allow_external_file: bool,
    environment: &Environment,
) -> Result<(Vec<PendingAttachment>, String)> {
    let mut pending = Vec::with_capacity(attachments.len());
    let mut stderr = String::new();
    for file_path in attachments {
        let trimmed = file_path.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let _ = writeln!(
                stderr,
                "Skipping --attachment {file_path:?}: URLs are not supported here, only local file paths."
            );
            continue;
        }
        let path = Path::new(file_path);
        if !allow_external_file {
            let base = fs::canonicalize(environment.current_dir())
                .unwrap_or_else(|_| lexical_normalize(environment.current_dir()));
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                environment.current_dir().join(path)
            };
            let candidate =
                fs::canonicalize(&absolute).unwrap_or_else(|_| lexical_normalize(&absolute));
            if !candidate.starts_with(&base) {
                bail!(
                    "--attachment path {file_path:?} resolves outside the current working directory; attach files generated inside the task workdir rather than machine-shared paths like /tmp, where another run's stale file can be attached by mistake. Pass --allow-external-file to override."
                );
            }
        }
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let data = fs::read(read_path).with_context(|| format!("read attachment {file_path}"))?;
        pending.push(PendingAttachment {
            path: file_path.clone(),
            data,
        });
    }
    Ok((pending, stderr))
}

fn active_duplicate_issue_message(error: &anyhow::Error) -> Option<String> {
    let error = error.downcast_ref::<HttpError>()?;
    if error.status_code != 409 {
        return None;
    }
    let payload: Value = serde_json::from_str(&error.body).ok()?;
    (payload.get("code").and_then(Value::as_str) == Some("active_duplicate_issue"))
        .then(|| {
            payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .filter(|message| !message.is_empty())
}

fn guard_issue_description_local_links(
    description: &str,
    environment: &Environment,
    remediation: &str,
) -> Result<()> {
    if !environment.in_agent_execution_context() {
        return Ok(());
    }
    let findings = find_runtime_local_markdown_links(description, environment.current_dir());
    if findings.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "issue description links {} runtime-local path(s), which no reader can open:\n",
        findings.len()
    );
    for (target, reason) in findings {
        let _ = writeln!(message, "  - {target:?} — {reason}");
    }
    message.push_str(
        "\nThe path exists only on the machine running you; for everyone else the link is dead. ",
    );
    message.push_str(remediation);
    message.push_str("\nTo merely reference a code location, use inline code instead of a link (`path/to/file.ts:42`) — code spans and fenced blocks are not checked.");
    bail!("{message}")
}

fn find_runtime_local_markdown_links(
    markdown: &str,
    current_dir: &Path,
) -> Vec<(String, &'static str)> {
    let mut candidates = Vec::new();
    let mut in_fence: Option<char> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let fence = trimmed
            .chars()
            .next()
            .filter(|character| matches!(character, '`' | '~'))
            .filter(|character| {
                trimmed
                    .chars()
                    .take_while(|value| value == character)
                    .count()
                    >= 3
            });
        if let Some(character) = fence {
            match in_fence {
                Some(open) if open == character => in_fence = None,
                None => in_fence = Some(character),
                _ => {}
            }
            continue;
        }
        if in_fence.is_some() || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        collect_inline_markdown_destinations(line, &mut candidates);
        if let Some((_, destination)) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("]:"))
        {
            if let Some(destination) = markdown_destination(destination.trim_start()) {
                candidates.push(destination);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut findings = Vec::new();
    for candidate in candidates {
        let target = candidate.trim().to_string();
        if target.is_empty() || !seen.insert(target.clone()) {
            continue;
        }
        if let Some(reason) = classify_runtime_local_target(&target, current_dir) {
            findings.push((target, reason));
        }
    }
    findings
}

fn collect_inline_markdown_destinations(line: &str, destinations: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let run = bytes[index..]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            index += run;
            while index < bytes.len() {
                let closing_run = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                if closing_run == run {
                    index += run;
                    break;
                }
                index += closing_run.max(1);
            }
            continue;
        }
        if bytes[index] == b'<' {
            if let Some(end) = line[index + 1..].find('>') {
                let target = &line[index + 1..index + 1 + end];
                if target.to_ascii_lowercase().starts_with("file://") {
                    destinations.push(target.into());
                }
                index += end + 2;
                continue;
            }
        }
        if bytes[index] == b']'
            && bytes.get(index + 1) == Some(&b'(')
            && !is_markdown_escaped(bytes, index)
        {
            let start = index + 2;
            if let Some(target) = markdown_destination(&line[start..]) {
                destinations.push(target);
            }
        }
        index += 1;
    }
}

fn is_markdown_escaped(bytes: &[u8], index: usize) -> bool {
    bytes[..index]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

fn markdown_destination(input: &str) -> Option<String> {
    let input = input.trim_start();
    if let Some(input) = input.strip_prefix('<') {
        return input.find('>').map(|end| input[..end].into());
    }
    let mut output = String::new();
    let mut depth = 0_usize;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            output.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => {
                depth += 1;
                output.push(character);
            }
            ')' if depth == 0 => break,
            ')' => {
                depth -= 1;
                output.push(character);
            }
            character if character.is_whitespace() && depth == 0 => break,
            _ => output.push(character),
        }
    }
    (!output.is_empty()).then_some(output)
}

fn classify_runtime_local_target(target: &str, current_dir: &Path) -> Option<&'static str> {
    let target = target.trim();
    let path = Path::new(target);
    if path.is_absolute() {
        let base = fs::canonicalize(current_dir).unwrap_or_else(|_| lexical_normalize(current_dir));
        let resolved = fs::canonicalize(path).unwrap_or_else(|_| lexical_normalize(path));
        if resolved.starts_with(base) {
            return Some("it is inside this task's working directory");
        }
        if fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
            return Some("it names a file that exists only on this machine");
        }
        return None;
    }
    Url::parse(target)
        .ok()
        .filter(|url| url.scheme().eq_ignore_ascii_case("file"))
        .map(|_| "it is a file:// URL")
}

async fn run_user_profile_get(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let profile: Value = client
        .get_json("/api/me")
        .await
        .context("get user profile")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&profile)?),
        OutputFormat::Table => format_user_profile_table(&profile),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_user_profile_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &UpdateProfileArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let description = resolve_profile_description(args, environment, input)?;
    let client = new_api_client(cli, environment)?;
    let profile: Value = client
        .patch_json(
            "/api/me",
            &serde_json::json!({"profile_description": description}),
        )
        .await
        .context("update user profile")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&profile)?),
        OutputFormat::Table => format_user_profile_table(&profile),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceSummary {
    id: String,
    name: String,
    slug: String,
}

async fn run_workspace_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let workspaces = fetch_workspaces(cli, environment).await?;
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&workspaces)?),
            stderr: String::new(),
        });
    }
    if workspaces.is_empty() {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: "No workspaces found.\n".into(),
        });
    }

    let current_id = resolve_current_workspace_id(cli, environment);
    let stdout = format_workspace_table(&workspaces, &current_id, full_id);
    let current_hint = if current_id.is_empty() {
        "\nNo default workspace set. Use 'cordy workspace switch <id|slug|prefix>' to pick one.\n"
    } else {
        "\n* = current default workspace (use 'cordy workspace switch <id|slug|prefix>' to change)\n"
    };
    Ok(RunOutput {
        stdout,
        stderr: format!(
            "{current_hint}Tip: pass the ID column, SLUG, or full UUID (--full-id) to 'workspace get/update/switch'.\n"
        ),
    })
}

async fn fetch_workspaces(cli: &Cli, environment: &Environment) -> Result<Vec<WorkspaceSummary>> {
    let client = new_unscoped_authenticated_api_client(cli, environment)?;
    client
        .get_json("/api/workspaces")
        .await
        .context("list workspaces")
}

async fn run_workspace_get(
    cli: &Cli,
    environment: &Environment,
    workspace: Option<&str>,
    output: OutputFormat,
) -> Result<RunOutput> {
    let workspace_id = resolve_workspace_arg(cli, environment, workspace).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    let workspace: Value = client
        .get_json(&format!("/api/workspaces/{workspace_id}"))
        .await
        .context("get workspace")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&workspace)?),
            OutputFormat::Table => format_workspace_details_table(&workspace),
        },
        stderr: String::new(),
    })
}

#[derive(Debug, Serialize)]
struct CreateWorkspaceBody {
    name: String,
    slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_prefix: Option<String>,
}

async fn run_workspace_create<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &CreateWorkspaceArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let body = build_workspace_create_body(args, input)?;
    let client = new_unscoped_api_client(cli, environment)?;
    let workspace: Value = client
        .post_json("/api/workspaces", &body)
        .await
        .context("create workspace")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&workspace)?),
            OutputFormat::Table => format_workspace_details_table(&workspace),
        },
        stderr: String::new(),
    })
}

fn build_workspace_create_body<R: Read>(
    args: &CreateWorkspaceArgs,
    input: &mut R,
) -> Result<CreateWorkspaceBody> {
    let name = args.name.as_deref().unwrap_or_default();
    if name.trim().is_empty() {
        bail!("--name is required");
    }
    let slug = args.slug.as_deref().unwrap_or_default();
    if slug.trim().is_empty() {
        bail!("--slug is required");
    }
    if args.description_stdin && args.context_stdin {
        bail!(
            "--description-stdin and --context-stdin cannot be combined; a single stdin cannot feed both fields — pass one of them inline"
        );
    }
    let description = resolve_optional_text_input(
        args.description.as_deref(),
        args.description_stdin,
        "description",
        input,
    )?;
    let context = resolve_optional_text_input(
        args.context.as_deref(),
        args.context_stdin,
        "context",
        input,
    )?;
    let issue_prefix = args
        .issue_prefix
        .as_ref()
        .map(|prefix| {
            if prefix.trim().is_empty() {
                bail!("--issue-prefix cannot be empty; omit it to use the server-generated prefix");
            }
            Ok(prefix.clone())
        })
        .transpose()?;
    Ok(CreateWorkspaceBody {
        name: name.into(),
        slug: slug.into(),
        description,
        context,
        issue_prefix,
    })
}

fn resolve_optional_text_input<R: Read>(
    inline: Option<&str>,
    use_stdin: bool,
    field: &str,
    input: &mut R,
) -> Result<Option<String>> {
    if use_stdin && inline.is_some_and(|value| !value.is_empty()) {
        bail!("--{field} and --{field}-stdin are mutually exclusive");
    }
    if use_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .with_context(|| format!("read stdin for --{field}-stdin"))?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --{field}-stdin is empty");
        }
        return Ok(Some(body));
    }
    Ok(inline.map(unescape_backslash_escapes))
}

async fn run_workspace_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &UpdateWorkspaceArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let body = build_workspace_update_body(args, environment, input)?;
    if body.is_empty() {
        bail!("no fields to update; use --name, --description, --context, or --issue-prefix");
    }
    let client = new_api_client(cli, environment)?;
    let workspace: Value = client
        .patch_json(&format!("/api/workspaces/{workspace_id}"), &body)
        .await
        .context("update workspace")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&workspace)?),
            OutputFormat::Table => format_workspace_details_table(&workspace),
        },
        stderr: String::new(),
    })
}

fn build_workspace_update_body<R: Read>(
    args: &UpdateWorkspaceArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<serde_json::Map<String, Value>> {
    if args.description_stdin && args.context_stdin {
        bail!(
            "--description-stdin and --context-stdin cannot be combined; a single stdin cannot feed both fields — pass one of them inline or by file"
        );
    }
    let mut body = serde_json::Map::new();
    if let Some(name) = &args.name {
        body.insert("name".into(), Value::String(name.clone()));
    }
    if let Some(description) = resolve_update_text_input(
        args.description.as_deref(),
        args.description_stdin,
        args.description_file.as_deref(),
        args.allow_external_file,
        "description",
        environment,
        input,
    )? {
        body.insert("description".into(), Value::String(description));
    }
    if let Some(context) = resolve_update_text_input(
        args.context.as_deref(),
        args.context_stdin,
        args.context_file.as_deref(),
        args.allow_external_file,
        "context",
        environment,
        input,
    )? {
        body.insert("context".into(), Value::String(context));
    }
    if let Some(issue_prefix) = &args.issue_prefix {
        if issue_prefix.trim().is_empty() {
            bail!("--issue-prefix cannot be empty; clearing the prefix is not supported");
        }
        body.insert("issue_prefix".into(), Value::String(issue_prefix.clone()));
    }
    Ok(body)
}

#[allow(clippy::too_many_arguments)]
fn resolve_update_text_input<R: Read>(
    inline: Option<&str>,
    use_stdin: bool,
    file: Option<&Path>,
    allow_external_file: bool,
    field: &str,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let sources = [use_stdin, inline.is_some(), file.is_some()]
        .into_iter()
        .filter(|source| *source)
        .count();
    if sources > 1 {
        bail!("--{field}, --{field}-stdin, and --{field}-file are mutually exclusive");
    }
    if use_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .with_context(|| format!("read stdin for --{field}-stdin"))?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --{field}-stdin is empty");
        }
        return Ok(Some(body));
    }
    if let Some(path) = file {
        ensure_file_within_workdir(path, environment.current_dir(), allow_external_file, field)?;
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(read_path).with_context(|| format!("read file for --{field}-file"))?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --{field}-file is empty");
        }
        return Ok(Some(body));
    }
    Ok(inline.map(unescape_backslash_escapes))
}

async fn resolve_workspace_arg(
    cli: &Cli,
    environment: &Environment,
    workspace: Option<&str>,
) -> Result<String> {
    let Some(workspace) = workspace else {
        return Ok(resolve_current_workspace_id(cli, environment));
    };
    let target = workspace.trim();
    if target.is_empty() {
        bail!("workspace id, slug, or id prefix is required");
    }
    if is_canonical_uuid(target) {
        return Ok(target.into());
    }
    let workspaces = fetch_workspaces(cli, environment).await?;
    Ok(resolve_workspace_reference(&workspaces, target)?.id.clone())
}

fn resolve_workspace_reference<'a>(
    workspaces: &'a [WorkspaceSummary],
    target: &str,
) -> Result<&'a WorkspaceSummary> {
    let target = target.trim();
    if target.is_empty() {
        bail!("workspace id, slug, or id prefix is required");
    }
    if let Some(workspace) = workspaces
        .iter()
        .find(|workspace| workspace.id.eq_ignore_ascii_case(target))
    {
        return Ok(workspace);
    }
    if let Some(workspace) = workspaces
        .iter()
        .find(|workspace| !workspace.slug.is_empty() && workspace.slug.eq_ignore_ascii_case(target))
    {
        return Ok(workspace);
    }
    if let Some(prefix) = normalize_uuid_prefix(target) {
        let matches: Vec<_> = workspaces
            .iter()
            .filter(|workspace| compact_uuid(&workspace.id).starts_with(&prefix))
            .collect();
        match matches.as_slice() {
            [workspace] => return Ok(workspace),
            [_, _, ..] => {
                let details = matches
                    .iter()
                    .map(|workspace| {
                        let label = if workspace.slug.is_empty() {
                            workspace.name.clone()
                        } else {
                            format!("{} ({})", workspace.name, workspace.slug)
                        };
                        format!("  {}  {label}", workspace.id)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!(
                    "ambiguous workspace id prefix {target:?}; matches:\n{details}\nUse more characters, the slug, or the full UUID"
                );
            }
            _ => {}
        }
    }
    bail!(
        "workspace {target:?} not found or you do not have access; run 'cordy workspace list' to see options"
    )
}

fn is_canonical_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn normalize_uuid_prefix(value: &str) -> Option<String> {
    let prefix = value.trim().replace('-', "").to_ascii_lowercase();
    (prefix.len() >= 4 && prefix.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(prefix)
}

fn compact_uuid(value: &str) -> String {
    value.trim().replace('-', "").to_ascii_lowercase()
}

fn format_workspace_details_table(workspace: &Value) -> String {
    let description = truncate_text(&value_string(workspace, "description"), 60);
    let context = truncate_text(&value_string(workspace, "context"), 60);
    format_table(&[
        vec![
            "ID".into(),
            "NAME".into(),
            "SLUG".into(),
            "DESCRIPTION".into(),
            "CONTEXT".into(),
        ],
        vec![
            value_string(workspace, "id"),
            value_string(workspace, "name"),
            value_string(workspace, "slug"),
            description,
            context,
        ],
    ])
}

fn truncate_text(value: &str, limit: usize) -> String {
    if value.chars().count() > limit {
        value.chars().take(limit - 3).collect::<String>() + "..."
    } else {
        value.into()
    }
}

fn format_table(rows: &[Vec<String>]) -> String {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or_default();
    let widths: Vec<_> = (0..column_count.saturating_sub(1))
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|value| value.chars().count())
                .max()
                .unwrap_or_default()
                + 2
        })
        .collect();
    let mut output = String::new();
    for row in rows {
        for (column, value) in row.iter().enumerate() {
            if let Some(width) = widths.get(column) {
                let _ = write!(output, "{value:<width$}");
            } else {
                output.push_str(value);
            }
        }
        output.push('\n');
    }
    output
}

fn format_workspace_table(
    workspaces: &[WorkspaceSummary],
    current_id: &str,
    full_id: bool,
) -> String {
    let mut rows = Vec::with_capacity(workspaces.len() + 1);
    rows.push([String::new(), "ID".into(), "NAME".into(), "SLUG".into()]);
    rows.extend(workspaces.iter().map(|workspace| {
        [
            (if workspace.id == current_id { "*" } else { " " }).into(),
            display_id(&workspace.id, full_id),
            workspace.name.clone(),
            workspace.slug.clone(),
        ]
    }));
    let widths: [usize; 3] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .max()
            .unwrap_or_default()
            + 2
    });
    let mut output = String::new();
    for row in rows {
        let _ = writeln!(
            output,
            "{:<marker_width$}{:<id_width$}{:<name_width$}{}",
            row[0],
            row[1],
            row[2],
            row[3],
            marker_width = widths[0],
            id_width = widths[1],
            name_width = widths[2]
        );
    }
    output
}

fn display_id(id: &str, full: bool) -> String {
    if full {
        id.into()
    } else {
        id.chars().take(8).collect()
    }
}

fn resolve_profile_description<R: Read>(
    args: &UpdateProfileArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<String> {
    let inline = args.description.as_deref().unwrap_or_default();
    let sources = [
        args.description_stdin,
        !inline.is_empty(),
        args.description_file.is_some(),
    ]
    .into_iter()
    .filter(|source| *source)
    .count();
    if sources > 1 {
        bail!("--description, --description-stdin, and --description-file are mutually exclusive");
    }

    let (description, has_description) = if args.description_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --description-stdin")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --description-stdin is empty");
        }
        (body, true)
    } else if let Some(path) = &args.description_file {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "description",
        )?;
        let read_path = if path.is_absolute() {
            path.clone()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(read_path).context("read file for --description-file")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --description-file is empty");
        }
        (body, true)
    } else if inline.is_empty() {
        (String::new(), false)
    } else {
        (unescape_backslash_escapes(inline), true)
    };

    if args.clear && has_description {
        bail!(
            "--clear cannot be combined with --description / --description-stdin / --description-file"
        );
    }
    if !args.clear && !has_description && args.description.is_none() {
        bail!(
            "nothing to update; pass --description, --description-stdin, --description-file, or --clear"
        );
    }
    Ok(if args.clear {
        String::new()
    } else {
        description
    })
}

fn trim_one_trailing_newline(mut value: String) -> String {
    if value.ends_with('\n') {
        value.pop();
    }
    value
}

fn unescape_backslash_escapes(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.peek().copied() {
            Some('n') => {
                chars.next();
                output.push('\n');
            }
            Some('r') => {
                chars.next();
                output.push('\r');
            }
            Some('t') => {
                chars.next();
                output.push('\t');
            }
            Some('\\') => {
                chars.next();
                output.push('\\');
            }
            _ => output.push('\\'),
        }
    }
    output
}

fn ensure_file_within_workdir(
    file_path: &Path,
    current_dir: &Path,
    allow_external_file: bool,
    field: &str,
) -> Result<()> {
    if allow_external_file {
        return Ok(());
    }
    let base = fs::canonicalize(current_dir).unwrap_or_else(|_| lexical_normalize(current_dir));
    let absolute = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        current_dir.join(file_path)
    };
    let candidate = fs::canonicalize(&absolute).unwrap_or_else(|_| {
        let parent = absolute.parent().unwrap_or(current_dir);
        let parent = fs::canonicalize(parent).unwrap_or_else(|_| lexical_normalize(parent));
        absolute
            .file_name()
            .map_or_else(|| lexical_normalize(&absolute), |name| parent.join(name))
    });
    if !candidate.starts_with(&base) {
        bail!(
            "--{field}-file path {:?} resolves outside the current working directory; write agent temp files inside the task workdir (e.g. ./{field}.md) rather than machine-shared paths like /tmp, where another run's stale file can be read by mistake. Pass --allow-external-file to override.",
            file_path,
        );
    }
    Ok(())
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn new_api_client(cli: &Cli, environment: &Environment) -> Result<ApiClient> {
    new_api_client_with_options(cli, environment, true, false, true)
}

fn new_unscoped_authenticated_api_client(
    cli: &Cli,
    environment: &Environment,
) -> Result<ApiClient> {
    new_api_client_with_options(cli, environment, false, true, false)
}

fn new_unscoped_api_client(cli: &Cli, environment: &Environment) -> Result<ApiClient> {
    new_api_client_with_options(cli, environment, false, false, true)
}

fn new_api_client_with_options(
    cli: &Cli,
    environment: &Environment,
    include_workspace: bool,
    require_token: bool,
    include_execution_context: bool,
) -> Result<ApiClient> {
    let task_context = environment.in_daemon_managed_execution_context();
    // A daemon task with no private config root must not even read the owner's
    // global profile. This mirrors the Go resolver's fail-closed boundary, not
    // merely its eventual choice of credentials.
    let may_read_config =
        !task_context || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some();
    let config = if may_read_config {
        environment.load_config(&cli.profile).unwrap_or_default()
    } else {
        config::CliConfig::default()
    };
    let token = environment
        .trimmed("CORDY_TOKEN")
        .map(ToOwned::to_owned)
        .or_else(|| (!task_context).then(|| config.token.clone()))
        .unwrap_or_default();
    if task_context && !token.starts_with("mat_") {
        let suffix = environment
            .leftover_marker_suffix()
            .unwrap_or_else(|| environment.daemon_port_only_context_hint().into());
        bail!(
            "agent execution context requires CORDY_TOKEN to be a task-scoped mat_ token{suffix}"
        );
    }
    let explicit_server_url = cli
        .server_url
        .as_deref()
        .or_else(|| environment.trimmed("CORDY_SERVER_URL"));
    let server_url = if let Some(raw) = explicit_server_url.filter(|value| !value.is_empty()) {
        normalize_api_base_url(raw).unwrap_or_else(|_| raw.into())
    } else if !task_context || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some() {
        if config.server_url.is_empty() {
            String::new()
        } else {
            normalize_api_base_url(&config.server_url).unwrap_or_else(|_| config.server_url.clone())
        }
    } else {
        String::new()
    };
    if server_url.is_empty() {
        bail!(
            "No server configured. Run 'cordy setup' first{}.",
            environment.daemon_port_only_context_hint()
        );
    }
    if require_token && token.is_empty() {
        bail!(
            "not authenticated: run 'cordy login' first{}",
            environment.daemon_port_only_context_hint()
        );
    }

    let workspace_id = if include_workspace {
        resolve_workspace_id(cli, environment, task_context, &config)
    } else {
        String::new()
    };
    ApiClient::new(
        server_url,
        workspace_id,
        token,
        if include_execution_context {
            environment.raw("CORDY_AGENT_ID").unwrap_or_default()
        } else {
            ""
        }
        .into(),
        if include_execution_context {
            environment.raw("CORDY_TASK_ID").unwrap_or_default()
        } else {
            ""
        }
        .into(),
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")),
        CLIENT_VERSION,
    )
}

fn resolve_current_workspace_id(cli: &Cli, environment: &Environment) -> String {
    let task_context = environment.in_daemon_managed_execution_context();
    let may_read_config =
        !task_context || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some();
    let config = if may_read_config {
        environment.load_config(&cli.profile).unwrap_or_default()
    } else {
        config::CliConfig::default()
    };
    resolve_workspace_id(cli, environment, task_context, &config)
}

fn resolve_workspace_id(
    cli: &Cli,
    environment: &Environment,
    task_context: bool,
    config: &config::CliConfig,
) -> String {
    match cli.workspace_id.as_deref() {
        Some(value) if !value.is_empty() => value.into(),
        // An explicitly empty flag suppresses the environment, just like
        // Cobra's Changed branch, then falls through to profile config.
        Some(_) => {
            if task_context {
                String::new()
            } else {
                config.workspace_id.clone()
            }
        }
        None => environment
            .trimmed("CORDY_WORKSPACE_ID")
            .map(Into::into)
            .or_else(|| (!task_context).then(|| config.workspace_id.clone()))
            .unwrap_or_default(),
    }
}

fn normalize_api_base_url(raw: &str) -> Result<String> {
    let mut url = Url::parse(raw.trim()).context("invalid CORDY_SERVER_URL")?;
    match url.scheme() {
        "ws" => url
            .set_scheme("http")
            .map_err(|_| anyhow::anyhow!("set scheme"))?,
        "wss" => url
            .set_scheme("https")
            .map_err(|_| anyhow::anyhow!("set scheme"))?,
        "http" | "https" => {}
        _ => bail!("CORDY_SERVER_URL must use ws, wss, http, or https"),
    }
    if url.path() == "/ws" {
        url.set_path("");
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string().trim_end_matches('/').into())
}

fn format_user_profile_table(profile: &Value) -> String {
    let values = [
        ("ID", value_string(profile, "id")),
        ("NAME", value_string(profile, "name")),
        ("EMAIL", value_string(profile, "email")),
        (
            "PROFILE DESCRIPTION",
            match value_string(profile, "profile_description") {
                value if value.is_empty() => "(not set)".into(),
                value => value,
            },
        ),
    ];
    let width = values
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0)
        + 2;
    let mut output = String::new();
    for (label, value) in values {
        let _ = writeln!(output, "{label:<width$}{value}");
    }
    output
}

fn value_string(object: &Value, key: &str) -> String {
    match object.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Request;
    use axum::http::HeaderMap;
    use axum::routing::{delete as delete_route, get, patch, post, put};
    use axum::{Json, Router};
    use clap::Parser;
    use std::fs;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    #[test]
    fn version_text_json_and_root_flag_match_go_contract() {
        let text = run_version(VersionOutput::Text).expect("text version");
        assert_eq!(
            text.stdout,
            format!(
                "cordy {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})\ngo: {BUILD_GO_VERSION}, os/arch: {BUILD_OS}/{BUILD_ARCH}\n"
            )
        );
        assert!(text.stderr.is_empty());

        let json = run_version(VersionOutput::Json).expect("JSON version");
        let info: Value = serde_json::from_str(&json.stdout).expect("version JSON");
        assert_eq!(info.as_object().expect("version object").len(), 6);
        assert_eq!(info["version"], CLIENT_VERSION);
        assert_eq!(info["commit"], BUILD_COMMIT);
        assert_eq!(info["date"], BUILD_DATE);
        assert_eq!(info["go"], BUILD_GO_VERSION);
        assert_eq!(info["os"], BUILD_OS);
        assert_eq!(info["arch"], BUILD_ARCH);

        let root = Cli::try_parse_from(["cordy", "--version"])
            .expect_err("--version exits after rendering");
        assert_eq!(root.kind(), clap::error::ErrorKind::DisplayVersion);
        assert_eq!(root.to_string(), format!("cordy {ROOT_LONG_VERSION}\n"));
        let first_line =
            format!("cordy {CLIENT_VERSION} (commit: {BUILD_COMMIT}, built: {BUILD_DATE})");
        assert_eq!(root.to_string().lines().next(), Some(first_line.as_str()));
    }

    #[test]
    fn version_subcommand_accepts_only_go_registry_output_values() {
        assert!(Cli::try_parse_from(["cordy", "version"]).is_ok());
        assert!(Cli::try_parse_from(["cordy", "version", "--output", "text"]).is_ok());
        assert!(Cli::try_parse_from(["cordy", "version", "--output", "json"]).is_ok());
        assert!(Cli::try_parse_from(["cordy", "version", "--output", "table"]).is_err());
    }

    async fn test_server() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/api/me",
            get(|request: Request| async move {
                assert_eq!(request.headers()["authorization"], "Bearer token-from-env");
                assert_eq!(request.headers()["x-workspace-id"], "workspace-from-env");
                assert_eq!(request.headers()["x-client-platform"], "cli");
                assert_eq!(
                    request.headers()["x-client-capabilities"],
                    "stable_attachment_urls"
                );
                axum::Json(serde_json::json!({
                    "id": "user-1",
                    "name": "Ada",
                    "email": "ada@example.com",
                    "profile_description": "Maintainer"
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (format!("http://{address}"), task)
    }

    async fn patch_test_server() -> (
        String,
        Arc<Mutex<Option<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let captured = Arc::new(Mutex::new(None));
        let captured_by_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/me",
            patch(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_handler);
                async move {
                    *captured.lock().expect("capture body") = Some(body.clone());
                    Json(serde_json::json!({
                        "id": "user-1",
                        "name": "Ada",
                        "email": "ada@example.com",
                        "profile_description": body["profile_description"]
                    }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (format!("http://{address}"), captured, task)
    }

    fn update_args(cli: &Cli) -> &UpdateProfileArgs {
        match &cli.command {
            Command::User(UserArgs {
                command:
                    UserCommand::Profile(ProfileArgs {
                        command: ProfileCommand::Update(args),
                    }),
            }) => args,
            _ => panic!("expected user profile update"),
        }
    }

    fn create_workspace_args(cli: &Cli) -> &CreateWorkspaceArgs {
        match &cli.command {
            Command::Workspace(WorkspaceArgs {
                command: WorkspaceCommand::Create(args),
            }) => args,
            _ => panic!("expected workspace create"),
        }
    }

    fn update_workspace_args(cli: &Cli) -> &UpdateWorkspaceArgs {
        match &cli.command {
            Command::Workspace(WorkspaceArgs {
                command: WorkspaceCommand::Update(args),
            }) => args,
            _ => panic!("expected workspace update"),
        }
    }

    fn issue_list_args(cli: &Cli) -> &IssueListArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::List(args),
            }) => args,
            _ => panic!("expected issue list"),
        }
    }

    fn issue_create_args(cli: &Cli) -> &IssueCreateArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Create(args),
            }) => args,
            _ => panic!("expected issue create"),
        }
    }

    fn issue_update_args(cli: &Cli) -> &IssueUpdateArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Update(args),
            }) => args,
            _ => panic!("expected issue update"),
        }
    }

    fn issue_assign_args(cli: &Cli) -> &IssueAssignArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Assign(args),
            }) => args,
            _ => panic!("expected issue assign"),
        }
    }

    fn issue_status_args(cli: &Cli) -> &IssueStatusArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Status(args),
            }) => args,
            _ => panic!("expected issue status"),
        }
    }

    fn issue_reorder_args(cli: &Cli) -> &IssueReorderArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Reorder(args),
            }) => args,
            _ => panic!("expected issue reorder"),
        }
    }

    fn issue_comment_add_args(cli: &Cli) -> &IssueCommentAddArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::Comment(IssueCommentArgs {
                        command: IssueCommentCommand::Add(args),
                    }),
            }) => args,
            _ => panic!("expected issue comment add"),
        }
    }

    fn issue_comment_list_args(cli: &Cli) -> &IssueCommentListArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::Comment(IssueCommentArgs {
                        command: IssueCommentCommand::List(args),
                    }),
            }) => args,
            _ => panic!("expected issue comment list"),
        }
    }

    fn issue_runs_args(cli: &Cli) -> &IssueRunsArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Runs(args),
            }) => args,
            _ => panic!("expected issue runs"),
        }
    }

    fn issue_run_messages_args(cli: &Cli) -> &IssueRunMessagesArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::RunMessages(args),
            }) => args,
            _ => panic!("expected issue run-messages"),
        }
    }

    fn issue_cancel_task_args(cli: &Cli) -> &IssueCancelTaskArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::CancelTask(args),
            }) => args,
            _ => panic!("expected issue cancel-task"),
        }
    }

    fn issue_usage_args(cli: &Cli) -> &IssueUsageArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Usage(args),
            }) => args,
            _ => panic!("expected issue usage"),
        }
    }

    fn issue_rerun_args(cli: &Cli) -> &IssueRerunArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Rerun(args),
            }) => args,
            _ => panic!("expected issue rerun"),
        }
    }

    fn issue_search_args(cli: &Cli) -> &IssueSearchArgs {
        match &cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Search(args),
            }) => args,
            _ => panic!("expected issue search"),
        }
    }

    #[test]
    fn issue_list_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "list",
            "--output",
            "json",
            "--full-id",
            "--status",
            "custom_status",
            "--priority",
            "urgent",
            "--assignee-id",
            "11111111-1111-1111-1111-111111111111",
            "--project",
            "abcd",
            "--metadata",
            "ready=true",
            "--metadata",
            "score=42",
            "--limit",
            "20",
            "--offset",
            "5",
            "--sort",
            "created_at",
            "--direction",
            "DESC",
        ])
        .expect("issue list CLI");
        let args = issue_list_args(&cli);
        assert_eq!(args.output, OutputFormat::Json);
        assert!(args.full_id);
        assert_eq!(args.status.as_deref(), Some("custom_status"));
        assert_eq!(args.priority.as_deref(), Some("urgent"));
        assert_eq!(args.project.as_deref(), Some("abcd"));
        assert_eq!(
            args.metadata,
            vec![String::from("ready=true"), String::from("score=42")]
        );
        assert_eq!((args.limit, args.offset), (20, 5));
        assert_eq!(args.sort.as_deref(), Some("created_at"));
        assert_eq!(args.direction.as_deref(), Some("DESC"));
    }

    #[test]
    fn issue_list_metadata_filter_infers_primitives_and_rejects_duplicates() {
        let encoded = build_metadata_filter(&[
            "ready=true".into(),
            "score=42".into(),
            "forced=\"42\"".into(),
            "label=alpha".into(),
        ])
        .expect("metadata filter");
        let filter: Value = serde_json::from_str(&encoded).expect("metadata JSON");
        assert_eq!(filter["ready"], Value::Bool(true));
        assert_eq!(filter["score"], 42);
        assert_eq!(filter["forced"], "42");
        assert_eq!(filter["label"], "alpha");

        let error = build_metadata_filter(&["ready=true".into(), "ready=false".into()])
            .expect_err("duplicate metadata key");
        assert!(error.to_string().contains("given more than once"));
        let error =
            build_metadata_filter(&["missing-separator".into()]).expect_err("metadata key=value");
        assert!(error.to_string().contains("key=value form"));
    }

    #[test]
    fn issue_list_has_more_uses_offset_and_returned_count() {
        assert!(issue_list_has_more(1, 1, 3));
        assert!(!issue_list_has_more(1, 2, 3));
        assert!(issue_list_has_more(0, 0, 1));
    }

    #[test]
    fn issue_list_table_matches_go_columns_full_id_dates_and_actor_fallback() {
        let issues = vec![serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "identifier": "CORD-18",
            "title": "Migrate CLI",
            "status": "in_progress",
            "priority": "high",
            "assignee_type": "agent",
            "assignee_id": "22222222-2222-2222-2222-222222222222",
            "start_date": "2026-08-23T10:11:12Z",
            "due_date": "2026-08-30T00:00:00Z"
        })];
        let actors = IssueActorNames(HashMap::from([(
            "agent:22222222-2222-2222-2222-222222222222".into(),
            "CordyBot".into(),
        )]));
        let table = format_issue_list_table(&issues, true, &actors);
        assert!(table.starts_with("KEY"));
        assert!(table.contains("ID"));
        assert!(table.contains("CORD-18"));
        assert!(table.contains("11111111-1111-1111-1111-111111111111"));
        assert!(table.contains("agent:CordyBot"));
        assert!(table.contains("2026-08-23"));
        assert!(table.contains("2026-08-30"));

        let fallback = format_issue_list_table(&issues, false, &IssueActorNames::default());
        assert!(fallback.contains("agent:22222222-2222-2222-2222-222222222222"));
        assert!(!fallback.lines().next().unwrap_or_default().contains(" ID "));
    }

    #[tokio::test]
    async fn issue_list_resolves_filters_and_sends_go_query_and_json_envelope() {
        let captured = Arc::new(Mutex::new(None::<String>));
        let captured_by_issues = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async {
                    Json(serde_json::json!([{
                        "user_id": "11111111-1111-1111-1111-111111111111",
                        "name": "Ada Lovelace",
                        "email": "ada@example.com"
                    }]))
                }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/projects",
                get(|| async {
                    Json(serde_json::json!({
                        "projects": [{
                            "id": "abcd0000-0000-0000-0000-000000000000",
                            "title": "Rust migration",
                            "status": "active"
                        }]
                    }))
                }),
            )
            .route(
                "/api/issues",
                get(move |request: Request| {
                    let captured = Arc::clone(&captured_by_issues);
                    async move {
                        assert_eq!(request.headers()["authorization"], "Bearer token-1");
                        assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                        *captured.lock().expect("capture query") =
                            request.uri().query().map(Into::into);
                        Json(serde_json::json!({
                            "issues": [{
                                "id": "issue-1",
                                "identifier": "CORD-18",
                                "title": "Migrate CLI"
                            }],
                            "total": 3
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "list",
            "--output",
            "json",
            "--status",
            "custom_status",
            "--priority",
            "high",
            "--assignee",
            "Ada",
            "--project",
            "abcd",
            "--metadata",
            "ready=true",
            "--limit",
            "2",
            "--offset",
            "1",
            "--sort",
            "created_at",
            "--direction",
            "DESC",
        ])
        .expect("issue list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("issue list");
        let envelope: Value = serde_json::from_str(&output.stdout).expect("list JSON");
        assert_eq!(envelope["total"], 3);
        assert_eq!(envelope["limit"], 2);
        assert_eq!(envelope["offset"], 1);
        assert_eq!(envelope["has_more"], Value::Bool(true));
        assert_eq!(envelope["issues"][0]["identifier"], "CORD-18");

        let query = captured
            .lock()
            .expect("captured query")
            .clone()
            .expect("query");
        let query = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        assert_eq!(query["workspace_id"], "workspace-1");
        assert_eq!(query["status"], "custom_status");
        assert_eq!(query["priority"], "high");
        assert_eq!(query["limit"], "2");
        assert_eq!(query["offset"], "1");
        assert_eq!(query["assignee_id"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(query["project_id"], "abcd0000-0000-0000-0000-000000000000");
        assert_eq!(query["metadata"], r#"{"ready":true}"#);
        assert_eq!(query["sort"], "created_at");
        assert_eq!(query["direction"], "desc");
        task.abort();
    }

    #[tokio::test]
    async fn issue_list_rejects_invalid_sort_direction_and_conflicting_assignee_flags() {
        let client = ApiClient::new(
            "http://127.0.0.1:1".into(),
            "workspace-1".into(),
            "token".into(),
            String::new(),
            String::new(),
            std::time::Duration::from_secs(1),
            CLIENT_VERSION,
        )
        .expect("client");
        for (argv, expected) in [
            (
                vec!["cordy", "issue", "list", "--sort", "nonsense"],
                "invalid --sort",
            ),
            (
                vec!["cordy", "issue", "list", "--direction", "desc"],
                "--direction requires --sort",
            ),
            (
                vec![
                    "cordy",
                    "issue",
                    "list",
                    "--sort",
                    "created_at",
                    "--direction",
                    "sideways",
                ],
                "invalid --direction",
            ),
            (
                vec![
                    "cordy",
                    "issue",
                    "list",
                    "--sort",
                    "position",
                    "--direction",
                    "asc",
                ],
                "--direction requires --sort",
            ),
            (
                vec![
                    "cordy",
                    "issue",
                    "list",
                    "--assignee",
                    "Ada",
                    "--assignee-id",
                    "11111111-1111-1111-1111-111111111111",
                ],
                "mutually exclusive",
            ),
        ] {
            let cli = Cli::try_parse_from(argv).expect("CLI");
            let error = build_issue_list_query(&client, "workspace-1", issue_list_args(&cli))
                .await
                .expect_err("validation error");
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn issue_get_parser_defaults_to_json_and_accepts_only_one_reference() {
        let cli = Cli::try_parse_from(["cordy", "issue", "get", "CORD-18"]).expect("issue get CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command: IssueCommand::Get { id, output },
            }) => {
                assert_eq!(id, "CORD-18");
                assert_eq!(output, OutputFormat::Json);
            }
            _ => panic!("expected issue get"),
        }
        assert!(Cli::try_parse_from(["cordy", "issue", "get"]).is_err());
        assert!(Cli::try_parse_from(["cordy", "issue", "get", "A-1", "B-2"]).is_err());
        assert!(
            Cli::try_parse_from(["cordy", "issue", "get", "CORD-18", "--output", "table"]).is_ok()
        );
    }

    #[tokio::test]
    async fn issue_ref_rejects_short_uuid_and_invalid_inputs_without_http() {
        let client = ApiClient::new(
            "http://127.0.0.1:1".into(),
            "workspace-1".into(),
            "token".into(),
            String::new(),
            String::new(),
            std::time::Duration::from_millis(50),
            CLIENT_VERSION,
        )
        .expect("client");
        for input in ["1881", "1881-a167", "1852"] {
            let error = resolve_issue_ref(&client, input)
                .await
                .expect_err("short prefix");
            assert!(error.to_string().contains("short UUID prefix"));
            assert!(error.to_string().contains("MUL-123"));
        }
        let error = resolve_issue_ref(&client, "not-an-id")
            .await
            .expect_err("invalid ref");
        assert!(error
            .to_string()
            .contains("not a recognized issue reference"));
        assert!(!error.to_string().contains("short UUID prefix"));
    }

    #[tokio::test]
    async fn issue_get_resolves_key_then_fetches_canonical_issue() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let first_hits = Arc::clone(&hits);
        let second_hits = Arc::clone(&hits);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || {
                    let hits = Arc::clone(&first_hits);
                    async move {
                        hits.lock().expect("hits").push("CORD-18".into());
                        Json(serde_json::json!({
                            "id": "11111111-1111-1111-1111-111111111111",
                            "identifier": "CORD-18",
                            "title": "Resolver response"
                        }))
                    }
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111",
                get(move |request: Request| {
                    let hits = Arc::clone(&second_hits);
                    async move {
                        assert_eq!(request.headers()["authorization"], "Bearer token-1");
                        assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                        hits.lock().expect("hits").push("canonical".into());
                        Json(serde_json::json!({
                            "id": "11111111-1111-1111-1111-111111111111",
                            "identifier": "CORD-18",
                            "title": "Canonical issue",
                            "description": "Full details"
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "get", "CORD-18"]).expect("issue get CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("issue get");
        let issue: Value = serde_json::from_str(&output.stdout).expect("issue JSON");
        assert_eq!(issue["title"], "Canonical issue");
        assert_eq!(issue["description"], "Full details");
        assert_eq!(
            *hits.lock().expect("hits"),
            vec![String::from("CORD-18"), String::from("canonical")]
        );
        task.abort();
    }

    #[test]
    fn issue_get_table_matches_go_detail_columns() {
        let issue = serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "identifier": "CORD-18",
            "title": "Migrate get",
            "status": "in_progress",
            "priority": "high",
            "assignee_type": "member",
            "assignee_id": "22222222-2222-2222-2222-222222222222",
            "start_date": "2026-08-24T10:00:00Z",
            "due_date": "2026-08-31T10:00:00Z",
            "description": "Preserve the complete description"
        });
        let actors = IssueActorNames(HashMap::from([(
            "member:22222222-2222-2222-2222-222222222222".into(),
            "Ada".into(),
        )]));
        let table = format_issue_get_table(&issue, &actors);
        assert!(table.starts_with("KEY"));
        assert!(table.contains("DESCRIPTION"));
        assert!(table.contains("CORD-18"));
        assert!(table.contains("member:Ada"));
        assert!(table.contains("2026-08-24"));
        assert!(table.contains("2026-08-31"));
        assert!(table.contains("Preserve the complete description"));
    }

    #[test]
    fn issue_pull_requests_parser_supports_go_name_alias_and_defaults() {
        for name in ["pull-requests", "prs"] {
            let cli = Cli::try_parse_from(["cordy", "issue", name, "CORD-18"])
                .expect("pull requests CLI");
            match cli.command {
                Command::Issue(IssueArgs {
                    command: IssueCommand::PullRequests { id, output },
                }) => {
                    assert_eq!(id, "CORD-18");
                    assert_eq!(output, OutputFormat::Table);
                }
                _ => panic!("expected issue pull-requests"),
            }
        }
        assert!(Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-requests",
            "CORD-18",
            "--output",
            "json"
        ])
        .is_ok());
    }

    #[tokio::test]
    async fn issue_pull_requests_resolves_issue_and_preserves_json_wrapper() {
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let resolve_hits = Arc::clone(&hits);
        let pull_request_hits = Arc::clone(&hits);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || {
                    let hits = Arc::clone(&resolve_hits);
                    async move {
                        hits.lock().expect("hits").push("resolve".into());
                        Json(serde_json::json!({
                            "id": "11111111-1111-1111-1111-111111111111",
                            "identifier": "CORD-18"
                        }))
                    }
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/pull-requests",
                get(move |request: Request| {
                    let hits = Arc::clone(&pull_request_hits);
                    async move {
                        assert_eq!(request.headers()["authorization"], "Bearer token-1");
                        assert_eq!(request.headers()["x-workspace-id"], "workspace-1");
                        hits.lock().expect("hits").push("pull-requests".into());
                        Json(serde_json::json!({
                            "pull_requests": [{
                                "number": 42,
                                "state": "open",
                                "title": "Rust CLI",
                                "url": "https://github.example/pr/42"
                            }],
                            "count": 1
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "prs", "CORD-18", "--output", "json"])
            .expect("pull requests CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("pull requests");
        let result: Value = serde_json::from_str(&output.stdout).expect("pull request JSON");
        assert_eq!(result["count"], 1);
        assert_eq!(result["pull_requests"][0]["number"], 42);
        assert_eq!(
            *hits.lock().expect("hits"),
            vec![String::from("resolve"), String::from("pull-requests")]
        );
        task.abort();
    }

    #[test]
    fn issue_pull_requests_table_uses_url_then_html_url_fallback() {
        let result = serde_json::json!({
            "pull_requests": [
                {
                    "number": 42,
                    "state": "open",
                    "title": "Direct URL",
                    "url": "https://github.example/pr/42",
                    "html_url": "https://ignored.example/pr/42"
                },
                {
                    "number": 43,
                    "state": "merged",
                    "title": "Fallback URL",
                    "html_url": "https://github.example/pr/43"
                }
            ]
        });
        let table = format_issue_pull_requests_table(&result);
        assert!(table.starts_with("NUMBER"));
        assert!(table.contains("Direct URL"));
        assert!(table.contains("https://github.example/pr/42"));
        assert!(!table.contains("https://ignored.example/pr/42"));
        assert!(table.contains("Fallback URL"));
        assert!(table.contains("https://github.example/pr/43"));
    }

    #[test]
    fn issue_pull_request_attach_parser_requires_url_and_matches_go_flags() {
        assert!(
            Cli::try_parse_from(["cordy", "issue", "pull-request", "attach", "CORD-18"]).is_err()
        );
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-request",
            "attach",
            "CORD-18",
            "--url",
            "https://github.com/owner/repo/pull/42",
            "--title",
            "Rust CLI",
            "--state",
            "open",
            "--branch",
            "cli",
            "--head-sha",
            "abc123",
            "--output",
            "json",
        ])
        .expect("attach CLI");
        match cli.command {
            Command::Issue(IssueArgs {
                command:
                    IssueCommand::PullRequest(IssuePullRequestArgs {
                        command: IssuePullRequestCommand::Attach(args),
                    }),
            }) => {
                assert_eq!(args.issue_id, "CORD-18");
                assert_eq!(args.url, "https://github.com/owner/repo/pull/42");
                assert_eq!(args.title.as_deref(), Some("Rust CLI"));
                assert_eq!(args.state.as_deref(), Some("open"));
                assert_eq!(args.branch.as_deref(), Some("cli"));
                assert_eq!(args.head_sha.as_deref(), Some("abc123"));
                assert_eq!(args.output, OutputFormat::Json);
            }
            _ => panic!("expected issue pull-request attach"),
        }
    }

    #[tokio::test]
    async fn issue_pull_request_attach_rejects_empty_url_with_go_guidance() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-request",
            "attach",
            "CORD-18",
            "--url",
            "",
        ])
        .expect("empty URL reaches runtime validation");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("empty URL");
        assert_eq!(
            error.to_string(),
            "--url is required (https://github.com/{owner}/{repo}/pull/{number})"
        );
    }

    #[tokio::test]
    async fn issue_pull_request_attach_posts_trimmed_url_and_optional_metadata() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_handler = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18"
                    }))
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/pull-requests",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_handler);
                    async move {
                        assert_eq!(headers["authorization"], "Bearer token-1");
                        *captured.lock().expect("capture body") = Some(body);
                        Json(serde_json::json!({
                            "pull_request": {
                                "number": 42,
                                "state": "open",
                                "title": "Rust CLI",
                                "url": "https://github.com/owner/repo/pull/42"
                            }
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "pull-request",
            "attach",
            "CORD-18",
            "--url",
            "  https://github.com/owner/repo/pull/42  ",
            "--title",
            "Rust CLI",
            "--state",
            "   ",
            "--branch",
            "cli",
            "--output",
            "json",
        ])
        .expect("attach CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("attach pull request");
        let result: Value = serde_json::from_str(&output.stdout).expect("attach JSON");
        assert_eq!(result["pull_request"]["number"], 42);
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("body");
        assert_eq!(body["url"], "https://github.com/owner/repo/pull/42");
        assert_eq!(body["title"], "Rust CLI");
        assert_eq!(body["branch"], "cli");
        assert!(body.get("state").is_none());
        assert!(body.get("head_sha").is_none());
        task.abort();
    }

    #[test]
    fn issue_children_parser_supports_alias_output_and_full_id_flag() {
        for name in ["children", "subissues"] {
            let cli = Cli::try_parse_from([
                "cordy",
                "issue",
                name,
                "CORD-18",
                "--output",
                "json",
                "--full-id",
            ])
            .expect("children CLI");
            match cli.command {
                Command::Issue(IssueArgs {
                    command:
                        IssueCommand::Children {
                            id,
                            output,
                            full_id,
                        },
                }) => {
                    assert_eq!(id, "CORD-18");
                    assert_eq!(output, OutputFormat::Json);
                    assert!(full_id);
                }
                _ => panic!("expected issue children"),
            }
        }
    }

    #[test]
    fn issue_children_sort_group_and_terminal_count_match_go() {
        let mut children = vec![
            serde_json::json!({"id":"u1","identifier":"CORD-4","stage":null,"status":"todo"}),
            serde_json::json!({"id":"s2a","identifier":"CORD-2","stage":2,"status":"cancelled","status_category":"cancelled"}),
            serde_json::json!({"id":"s1a","identifier":"CORD-1","stage":1,"status":"gate_approved","status_category":"done"}),
            serde_json::json!({"id":"s2b","identifier":"CORD-3","stage":2,"status":"in_progress","status_category":"in_progress"}),
            serde_json::json!({"id":"u2","identifier":"CORD-5","status":"done"}),
        ];
        children.sort_by_key(|child| child_stage(child).map_or((true, 0), |stage| (false, stage)));
        let identifiers = children
            .iter()
            .map(|child| value_string(child, "identifier"))
            .collect::<Vec<_>>();
        assert_eq!(
            identifiers,
            vec![
                String::from("CORD-1"),
                String::from("CORD-2"),
                String::from("CORD-3"),
                String::from("CORD-4"),
                String::from("CORD-5"),
            ]
        );
        let grouped = serde_json::to_value(group_issue_children(&children)).expect("group JSON");
        assert_eq!(grouped["total"], 5);
        assert_eq!(grouped["stages"][0]["stage"], 1);
        assert_eq!(grouped["stages"][0]["total"], 1);
        assert_eq!(grouped["stages"][0]["done"], 1);
        assert_eq!(grouped["stages"][1]["stage"], 2);
        assert_eq!(grouped["stages"][1]["total"], 2);
        assert_eq!(grouped["stages"][1]["done"], 1);
        assert_eq!(grouped["unstaged"].as_array().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn issue_children_resolves_parent_and_fetches_children_endpoint() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({
                        "id": "11111111-1111-1111-1111-111111111111",
                        "identifier": "CORD-18"
                    }))
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/children",
                get(|request: Request| async move {
                    assert_eq!(request.headers()["authorization"], "Bearer token-1");
                    Json(serde_json::json!({
                        "issues": [
                            {"id":"child-2","identifier":"CORD-20","stage":2,"status":"todo"},
                            {"id":"child-1","identifier":"CORD-19","stage":1,"status":"done"}
                        ]
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["cordy", "issue", "children", "CORD-18", "--output", "json"])
                .expect("children CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("children");
        let grouped: Value = serde_json::from_str(&output.stdout).expect("children JSON");
        assert_eq!(grouped["stages"][0]["stage"], 1);
        assert_eq!(grouped["stages"][1]["stage"], 2);
        assert_eq!(grouped["stages"][0]["done"], 1);
        task.abort();
    }

    #[test]
    fn issue_children_table_renders_stage_key_and_actor() {
        let children = vec![serde_json::json!({
            "id": "child-1",
            "identifier": "CORD-19",
            "stage": 1,
            "title": "First barrier",
            "status": "in_progress",
            "priority": "high",
            "assignee_type": "agent",
            "assignee_id": "agent-1"
        })];
        let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CordyBot".into())]));
        let table = format_issue_children_table(&children, &actors);
        assert!(table.starts_with("STAGE"));
        assert!(table.contains("CORD-19"));
        assert!(table.contains("First barrier"));
        assert!(table.contains("agent:CordyBot"));
    }

    #[test]
    fn issue_create_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "New issue",
            "--description",
            "Line 1\\nLine 2",
            "--status",
            "custom_status",
            "--priority",
            "high",
            "--assignee-id",
            "11111111-1111-1111-1111-111111111111",
            "--parent",
            "CORD-1",
            "--stage",
            "2",
            "--project",
            "abcd",
            "--start-date",
            "2026-08-24",
            "--due-date",
            "2026-08-31",
            "--allow-duplicate",
            "--attachment",
            "one.png",
            "--attachment",
            "two.png",
            "--attachment-id",
            "attachment-1",
            "--output",
            "table",
        ])
        .expect("issue create CLI");
        let args = issue_create_args(&cli);
        assert_eq!(args.title.as_deref(), Some("New issue"));
        assert_eq!(args.description.as_deref(), Some("Line 1\\nLine 2"));
        assert_eq!(args.status.as_deref(), Some("custom_status"));
        assert_eq!(args.priority.as_deref(), Some("high"));
        assert_eq!(args.stage, Some(2));
        assert_eq!(args.start_date.as_deref(), Some("2026-08-24"));
        assert_eq!(args.due_date.as_deref(), Some("2026-08-31"));
        assert!(args.allow_duplicate);
        assert_eq!(args.attachment.len(), 2);
        assert_eq!(args.attachment_id, vec![String::from("attachment-1")]);
        assert_eq!(args.output, OutputFormat::Table);
    }

    #[test]
    fn issue_create_description_modes_preserve_go_input_semantics() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let inline = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description",
            "one\\ntwo",
        ])
        .expect("inline CLI");
        assert_eq!(
            resolve_issue_create_description(
                issue_create_args(&inline),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("inline description"),
            Some("one\ntwo".into())
        );

        let stdin = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description-stdin",
        ])
        .expect("stdin CLI");
        assert_eq!(
            resolve_issue_create_description(
                issue_create_args(&stdin),
                &environment,
                &mut Cursor::new(b"literal\\nvalue\n".to_vec())
            )
            .expect("stdin description"),
            Some("literal\\nvalue".into())
        );

        let conflict = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description",
            "text",
            "--description-stdin",
        ])
        .expect("conflict reaches runtime");
        let error = resolve_issue_create_description(
            issue_create_args(&conflict),
            &environment,
            &mut Cursor::new(b"stdin".to_vec()),
        )
        .expect_err("mutually exclusive sources");
        assert!(error.to_string().contains("mutually exclusive"));

        let empty_file = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "T",
            "--description",
            "text",
            "--description-file",
            "",
        ])
        .expect("empty file flag reaches runtime");
        assert_eq!(
            resolve_issue_create_description(
                issue_create_args(&empty_file),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("empty file value is unset"),
            Some("text".into())
        );
    }

    #[test]
    fn issue_create_local_link_guard_is_agent_only_and_ignores_code() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let artifact = cwd.path().join("artifact.png");
        fs::write(&artifact, b"image").expect("artifact");
        let markdown = format!("[result]({})", artifact.display());

        let human = Environment::for_test(home.path().into(), cwd.path().into());
        let remediation = "Deliver it with `cordy issue create --attachment <path>`.";
        guard_issue_description_local_links(&markdown, &human, remediation)
            .expect("human links are allowed");

        let mut agent = Environment::for_test(home.path().into(), cwd.path().into());
        agent.set("CORDY_AGENT_ID", "agent-1");
        let error = guard_issue_description_local_links(&markdown, &agent, remediation)
            .expect_err("agent local link");
        assert!(error.to_string().contains("runtime-local path"));
        assert!(error.to_string().contains("--attachment"));
        guard_issue_description_local_links(
            &format!(
                "`[result]({})`\n```md\n[result]({})\n```",
                artifact.display(),
                artifact.display()
            ),
            &agent,
            remediation,
        )
        .expect("code spans and fences are ignored");
    }

    #[tokio::test]
    async fn issue_create_resolves_references_and_sends_complete_body() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_issue = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-10",
                get(|| async { Json(serde_json::json!({"id":"parent-uuid","identifier":"CORD-10"})) }),
            )
            .route(
                "/api/projects",
                get(|| async { Json(serde_json::json!({"projects":[{"id":"abcd0000-0000-0000-0000-000000000000","title":"Migration","status":"active"}]})) }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(serde_json::json!([{"user_id":"11111111-1111-1111-1111-111111111111","name":"Ada","email":"ada@example.com"}])) }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/issues",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_issue);
                    async move {
                        assert_eq!(headers["authorization"], "Bearer token-1");
                        *captured.lock().expect("capture issue") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"issue-uuid","identifier":"CORD-18","title":body["title"],
                            "status":body["status"],"priority":body["priority"]
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        environment.set("CORDY_QUICK_CREATE_TASK_ID", "task-quick");
        environment.set(
            "CORDY_QUICK_CREATE_ATTACHMENT_IDS",
            r#"["attachment-env","attachment-shared"]"#,
        );
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "New issue",
            "--description",
            "Line 1\\nLine 2",
            "--status",
            "custom_status",
            "--priority",
            "high",
            "--parent",
            "CORD-10",
            "--stage",
            "2",
            "--project",
            "abcd",
            "--assignee",
            "Ada",
            "--start-date",
            "2026-08-24",
            "--due-date",
            "2026-08-31",
            "--allow-duplicate",
            "--attachment-id",
            "attachment-flag",
            "--attachment-id",
            "attachment-shared",
        ])
        .expect("create CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create issue");
        let issue: Value = serde_json::from_str(&output.stdout).expect("issue JSON");
        assert_eq!(issue["identifier"], "CORD-18");
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["title"], "New issue");
        assert_eq!(body["description"], "Line 1\nLine 2");
        assert_eq!(body["status"], "custom_status");
        assert_eq!(body["priority"], "high");
        assert_eq!(body["parent_issue_id"], "parent-uuid");
        assert_eq!(body["stage"], 2);
        assert_eq!(body["project_id"], "abcd0000-0000-0000-0000-000000000000");
        assert_eq!(body["assignee_type"], "member");
        assert_eq!(body["assignee_id"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(body["start_date"], "2026-08-24");
        assert_eq!(body["due_date"], "2026-08-31");
        assert_eq!(body["allow_duplicate"], Value::Bool(true));
        assert_eq!(body["origin_type"], "quick_create");
        assert_eq!(body["origin_id"], "task-quick");
        assert_eq!(
            body["attachment_ids"],
            serde_json::json!(["attachment-flag", "attachment-shared", "attachment-env"])
        );
        task.abort();
    }

    #[tokio::test]
    async fn issue_create_surfaces_active_duplicate_message_verbatim() {
        let expected = "Active duplicate issue exists: CORD-1 Existing (status: in_progress).";
        let app = Router::new().route(
            "/api/issues",
            post(move || async move {
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(serde_json::json!({"code":"active_duplicate_issue","error":expected})),
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "create", "--title", "Duplicate"])
            .expect("create CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("duplicate");
        assert_eq!(error.to_string(), expected);
        task.abort();
    }

    #[tokio::test]
    async fn issue_create_prevalidates_attachments_and_treats_upload_failure_as_partial_success() {
        let issue_posts = Arc::new(Mutex::new(0_usize));
        let uploads = Arc::new(Mutex::new(0_usize));
        let issue_posts_by_handler = Arc::clone(&issue_posts);
        let uploads_by_handler = Arc::clone(&uploads);
        let app = Router::new()
            .route(
                "/api/issues",
                post(move || {
                    let posts = Arc::clone(&issue_posts_by_handler);
                    async move {
                        *posts.lock().expect("posts") += 1;
                        Json(serde_json::json!({"id":"issue-1","identifier":"CORD-1","title":"With file","status":"todo","priority":"none"}))
                    }
                }),
            )
            .route(
                "/api/upload-file",
                post(move |headers: HeaderMap, _body: axum::body::Bytes| {
                    let uploads = Arc::clone(&uploads_by_handler);
                    async move {
                        *uploads.lock().expect("uploads") += 1;
                        assert!(headers["content-type"].to_str().expect("content type").starts_with("multipart/form-data; boundary="));
                        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "upload failed")
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("good.png"), b"image").expect("attachment");
        let external = tempfile::tempdir().expect("external");
        let external_file = external.path().join("bad.png");
        fs::write(&external_file, b"bad").expect("external attachment");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let invalid = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "Invalid",
            "--attachment",
            external_file.to_str().expect("external path"),
        ])
        .expect("invalid attachment CLI");
        let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("external attachment");
        assert!(error.to_string().contains("--allow-external-file"));
        assert_eq!(*issue_posts.lock().expect("posts"), 0);
        assert_eq!(*uploads.lock().expect("uploads"), 0);

        let valid = Cli::try_parse_from([
            "cordy",
            "issue",
            "create",
            "--title",
            "With file",
            "--attachment",
            "good.png",
        ])
        .expect("attachment CLI");
        let output = run_with_input(&valid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("partial success");
        assert_eq!(*issue_posts.lock().expect("posts"), 1);
        assert_eq!(*uploads.lock().expect("uploads"), 1);
        assert!(output.stderr.contains("issue already created, CORD-1"));
        task.abort();
    }

    #[test]
    fn issue_update_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "update",
            "CORD-18",
            "--title",
            "Updated",
            "--description",
            "one\\ntwo",
            "--status",
            "in_review",
            "--priority",
            "urgent",
            "--assignee-id",
            "11111111-1111-1111-1111-111111111111",
            "--project",
            "",
            "--start-date",
            "",
            "--due-date",
            "2026-08-31",
            "--parent",
            "",
            "--stage",
            "2",
            "--position",
            "1.5",
            "--no-start",
            "--output",
            "table",
        ])
        .expect("issue update CLI");
        let args = issue_update_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.title.as_deref(), Some("Updated"));
        assert_eq!(args.description.as_deref(), Some("one\\ntwo"));
        assert_eq!(args.project.as_deref(), Some(""));
        assert_eq!(args.start_date.as_deref(), Some(""));
        assert_eq!(args.parent.as_deref(), Some(""));
        assert_eq!(args.stage, Some(2));
        assert_eq!(args.position, Some(1.5));
        assert!(args.no_start);
        assert_eq!(args.output, OutputFormat::Table);
    }

    #[tokio::test]
    async fn issue_update_rejects_invalid_enums_before_client_creation() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "update", "CORD-18", "--priority", "P1"])
            .expect("update CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("priority is rejected locally");
        assert!(error.to_string().contains("valid values"));
    }

    #[tokio::test]
    async fn issue_update_resolves_references_and_puts_only_changed_fields() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
            )
            .route(
                "/api/issues/PARENT-1",
                get(|| async { Json(serde_json::json!({"id":"parent-uuid","identifier":"CORD-1"})) }),
            )
            .route(
                "/api/projects",
                get(|| async { Json(serde_json::json!({"projects":[{"id":"abcd0000-0000-0000-0000-000000000000","title":"Migration","status":"active"}]})) }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(serde_json::json!([{"user_id":"member-uuid","name":"Ada","email":"ada@example.com"}])) }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/issues/issue-uuid",
                put(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        assert_eq!(headers["authorization"], "Bearer token-1");
                        *captured.lock().expect("capture update") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"issue-uuid","identifier":"CORD-18","title":body["title"],
                            "status":body["status"],"priority":body["priority"]
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "update",
            "CORD-18",
            "--title",
            "Updated",
            "--description",
            "one\\ntwo",
            "--status",
            "in_review",
            "--priority",
            "urgent",
            "--assignee",
            "Ada",
            "--project",
            "abcd",
            "--start-date",
            "",
            "--due-date",
            "2026-08-31",
            "--parent",
            "PARENT-1",
            "--stage",
            "2",
            "--position",
            "1.5",
            "--no-start",
            "--output",
            "table",
        ])
        .expect("update CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update issue");
        assert!(output.stdout.starts_with("KEY"));
        assert!(output.stdout.contains("CORD-18"));
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["title"], "Updated");
        assert_eq!(body["description"], "one\ntwo");
        assert_eq!(body["status"], "in_review");
        assert_eq!(body["priority"], "urgent");
        assert_eq!(body["assignee_type"], "member");
        assert_eq!(body["assignee_id"], "member-uuid");
        assert_eq!(body["project_id"], "abcd0000-0000-0000-0000-000000000000");
        assert_eq!(body["start_date"], "");
        assert_eq!(body["due_date"], "2026-08-31");
        assert_eq!(body["parent_issue_id"], "parent-uuid");
        assert_eq!(body["stage"], 2);
        assert_eq!(body["position"], 1.5);
        assert_eq!(body["suppress_run"], true);
        task.abort();
    }

    #[tokio::test]
    async fn issue_update_supports_explicit_clears_and_rejects_no_changes() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid",
                put(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        *captured.lock().expect("capture update") = Some(body);
                        Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let clear = Cli::try_parse_from([
            "cordy",
            "issue",
            "update",
            "CORD-18",
            "--description",
            "",
            "--project",
            "",
            "--parent",
            "",
        ])
        .expect("clear CLI");
        run_with_input(&clear, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("clear fields");
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["description"], "");
        assert_eq!(body["project_id"], Value::Null);
        assert_eq!(body["parent_issue_id"], Value::Null);

        let no_changes =
            Cli::try_parse_from(["cordy", "issue", "update", "CORD-18"]).expect("no changes CLI");
        let error = run_with_input(
            &no_changes,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect_err("no fields");
        assert!(error.to_string().contains("no fields to update"));
        task.abort();
    }

    #[tokio::test]
    async fn issue_assign_parser_and_local_validation_match_go() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--to-id",
            "11111111-1111-1111-1111-111111111111",
            "--no-start",
            "--output",
            "table",
        ])
        .expect("assign CLI");
        let args = issue_assign_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(
            args.to_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert!(args.no_start);
        assert_eq!(args.output, OutputFormat::Table);

        let missing = Cli::try_parse_from(["cordy", "issue", "assign", "CORD-18"])
            .expect("validation is at runtime");
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let error = run_with_input(&missing, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing target");
        assert!(error.to_string().contains("provide --to"));
    }

    #[tokio::test]
    async fn issue_assign_puts_resolved_actor_and_supports_unassign() {
        let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
        let bodies_by_update = Arc::clone(&bodies);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(serde_json::json!([])) }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(serde_json::json!([{"id":"11111111-1111-1111-1111-111111111111","name":"CodeBot"}])) }),
            )
            .route("/api/squads", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/issues/issue-uuid",
                put(move |Json(body): Json<Value>| {
                    let bodies = Arc::clone(&bodies_by_update);
                    async move {
                        bodies.lock().expect("bodies").push(body);
                        Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let assign = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--to-id",
            "11111111-1111-1111-1111-111111111111",
            "--no-start",
        ])
        .expect("assign CLI");
        let output = run_with_input(&assign, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("assign");
        assert!(output.stderr.contains("assigned to agent:CodeBot"));
        let assign_body = bodies.lock().expect("bodies")[0].clone();
        assert_eq!(assign_body["assignee_type"], "agent");
        assert_eq!(
            assign_body["assignee_id"],
            "11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(assign_body["suppress_run"], true);

        let unassign = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--unassign",
            "--output",
            "table",
        ])
        .expect("unassign CLI");
        let output = run_with_input(&unassign, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("unassign");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Issue CORD-18 unassigned.\n");
        let unassign_body = bodies.lock().expect("bodies")[1].clone();
        assert_eq!(unassign_body["assignee_type"], Value::Null);
        assert_eq!(unassign_body["assignee_id"], Value::Null);
        task.abort();
    }

    #[tokio::test]
    async fn issue_assign_rejects_no_start_with_unassign_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "assign",
            "CORD-18",
            "--unassign",
            "--no-start",
        ])
        .expect("assign CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("invalid no-start unassign");
        assert!(error.to_string().contains("--no-start"));
    }

    #[test]
    fn issue_status_parser_matches_go_registry_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "status",
            "CORD-18",
            "custom_status",
            "--no-start",
            "--output",
            "json",
        ])
        .expect("status CLI");
        let args = issue_status_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.status, "custom_status");
        assert!(args.no_start);
        assert_eq!(args.output, OutputFormat::Json);
    }

    #[tokio::test]
    async fn issue_status_validates_then_puts_status_and_suppress_run() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async { Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"})) }),
            )
            .route(
                "/api/issues/issue-uuid",
                put(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        *captured.lock().expect("capture status") = Some(body);
                        Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18","status":"custom_status"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "status",
            "CORD-18",
            "custom_status",
            "--no-start",
            "--output",
            "json",
        ])
        .expect("status CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("status update");
        assert_eq!(
            output.stderr,
            "Issue CORD-18 status changed to custom_status.\n"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&output.stdout).expect("status JSON")["status"],
            "custom_status"
        );
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["status"], "custom_status");
        assert_eq!(body["suppress_run"], true);
        task.abort();
    }

    #[tokio::test]
    async fn issue_status_rejects_malformed_status_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "status", "CORD-18", "not a status"])
            .expect("status CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("malformed status");
        assert!(error.to_string().contains("status key"));
    }

    #[test]
    fn issue_reorder_parser_enforces_exactly_one_real_target() {
        assert!(Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18"]).is_err());
        assert!(
            Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--top", "--bottom"])
                .is_err()
        );
        let cli = Cli::try_parse_from([
            "cordy", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
        ])
        .expect("reorder CLI");
        let args = issue_reorder_args(&cli);
        assert_eq!(args.id, "CORD-18");
        assert_eq!(args.before.as_deref(), Some("CORD-1"));
        assert_eq!(args.output, OutputFormat::Table);

        let false_top =
            Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--top=false"])
                .expect("false bool reaches runtime");
        assert_eq!(issue_reorder_args(&false_top).top, Some(false));
    }

    #[test]
    fn issue_reorder_position_math_matches_board_drag_contract() {
        let positions = HashMap::from([
            (String::from("one"), 10.0),
            (String::from("two"), 20.0),
            (String::from("three"), 40.0),
        ]);
        assert_eq!(
            compute_reorder_position(
                &["two".into(), "one".into(), "three".into()],
                "two",
                &positions,
                20.0,
            ),
            9.0
        );
        assert_eq!(
            compute_reorder_position(
                &["one".into(), "two".into(), "three".into()],
                "two",
                &positions,
                20.0,
            ),
            25.0
        );
        assert_eq!(
            compute_reorder_position(
                &["one".into(), "three".into(), "two".into()],
                "two",
                &positions,
                20.0,
            ),
            41.0
        );
    }

    #[tokio::test]
    async fn issue_reorder_paginates_project_column_and_puts_computed_position() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_update = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"target-id","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/CORD-1",
                get(|| async { Json(serde_json::json!({"id":"other-id","identifier":"CORD-1"})) }),
            )
            .route(
                "/api/issues/target-id",
                get(|| async {
                    Json(serde_json::json!({
                        "id":"target-id","identifier":"CORD-18","title":"Target",
                        "status":"todo","priority":"high","project_id":"project-1","position":20.0
                    }))
                })
                .put(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_update);
                    async move {
                        *captured.lock().expect("capture reorder") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"target-id","identifier":"CORD-18","title":"Target",
                            "status":"todo","priority":"high","position":body["position"]
                        }))
                    }
                }),
            )
            .route(
                "/api/issues",
                get(|request: Request| async move {
                    let query = request.uri().query().unwrap_or_default();
                    assert!(query.contains("workspace_id=workspace-1"));
                    assert!(query.contains("status=todo"));
                    assert!(query.contains("project_id=project-1"));
                    assert!(query.contains("sort=position"));
                    if query.contains("offset=0") {
                        Json(serde_json::json!({
                            "issues":[
                                {"id":"other-id","position":10.0},
                                {"id":"target-id","position":20.0}
                            ],
                            "total":3
                        }))
                    } else {
                        assert!(query.contains("offset=2"));
                        Json(serde_json::json!({
                            "issues":[{"id":"last-id","position":30.0}],
                            "total":3
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "reorder", "CORD-18", "--before", "CORD-1", "--output", "table",
        ])
        .expect("reorder CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("reorder issue");
        assert_eq!(output.stderr, "Issue CORD-18 reordered.\n");
        assert!(output.stdout.starts_with("KEY"));
        assert_eq!(
            captured
                .lock()
                .expect("body")
                .clone()
                .expect("captured body")["position"],
            9.0
        );
        task.abort();
    }

    #[tokio::test]
    async fn issue_reorder_rejects_false_selector_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "reorder", "CORD-18", "--bottom=false"])
            .expect("false bool reaches runtime");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("false selector");
        assert!(error.to_string().contains("cannot be set to false"));
    }

    #[test]
    fn issue_comment_add_parser_and_content_sources_match_go() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "add",
            "CORD-18",
            "--content",
            "one\\ntwo",
            "--parent",
            "comment-1",
            "--attachment",
            "one.png",
            "--output",
            "table",
        ])
        .expect("comment add CLI");
        let args = issue_comment_add_args(&cli);
        assert_eq!(args.issue_id, "CORD-18");
        assert_eq!(args.parent.as_deref(), Some("comment-1"));
        assert_eq!(args.attachment, vec![String::from("one.png")]);
        assert_eq!(args.output, OutputFormat::Table);
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        assert_eq!(
            resolve_issue_comment_content(args, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .expect("inline content"),
            Some("one\ntwo".into())
        );

        let empty_file = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "add",
            "CORD-18",
            "--content-file",
            "",
        ])
        .expect("empty file reaches runtime");
        assert!(resolve_issue_comment_content(
            issue_comment_add_args(&empty_file),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("empty file is unset")
        .is_none());
    }

    #[tokio::test]
    async fn issue_comment_add_prevalidates_uploads_then_posts_attachment_ids() {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_by_comment = Arc::clone(&captured);
        let uploads = Arc::new(Mutex::new(0_usize));
        let uploads_by_handler = Arc::clone(&uploads);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/upload-file",
                post(move |headers: HeaderMap, _body: axum::body::Bytes| {
                    let uploads = Arc::clone(&uploads_by_handler);
                    async move {
                        *uploads.lock().expect("uploads") += 1;
                        assert!(headers["content-type"]
                            .to_str()
                            .expect("content type")
                            .starts_with("multipart/form-data; boundary="));
                        Json(serde_json::json!({"id":"attachment-1"}))
                    }
                }),
            )
            .route(
                "/api/issues/issue-uuid/comments",
                post(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_comment);
                    async move {
                        *captured.lock().expect("comment body") = Some(body.clone());
                        Json(serde_json::json!({"id":"comment-1","content":body["content"]}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("proof.txt"), b"proof").expect("attachment");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "add",
            "CORD-18",
            "--content",
            "Completed\\nSee proof.",
            "--parent",
            "parent-comment",
            "--attachment",
            "proof.txt",
        ])
        .expect("comment add CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add comment");
        assert!(output.stderr.contains("Uploaded proof.txt"));
        assert!(output.stderr.contains("Comment added to issue CORD-18."));
        assert_eq!(*uploads.lock().expect("uploads"), 1);
        let body = captured
            .lock()
            .expect("body")
            .clone()
            .expect("captured body");
        assert_eq!(body["content"], "Completed\nSee proof.");
        assert_eq!(body["parent_id"], "parent-comment");
        assert_eq!(body["attachment_ids"], serde_json::json!(["attachment-1"]));
        task.abort();
    }

    #[tokio::test]
    async fn issue_comment_add_rejects_missing_content_before_network() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "issue", "comment", "add", "CORD-18"])
            .expect("missing content reaches runtime");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing content");
        assert!(error.to_string().contains("--content-file is required"));
    }

    #[tokio::test]
    async fn issue_comment_delete_resolve_and_unresolve_match_go_http_contracts() {
        let app = Router::new()
            .route(
                "/api/comments/comment-1",
                delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/comments/comment-1/resolve",
                post(|| async {
                    Json(serde_json::json!({"id":"comment-1","resolved_at":"2026-08-24T00:00:00Z"}))
                })
                .delete(|| async {
                    Json(serde_json::json!({"id":"comment-1","resolved_at":null}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let delete = Cli::try_parse_from(["cordy", "issue", "comment", "delete", "comment-1"])
            .expect("delete CLI");
        let output = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete comment");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Comment comment-1 deleted.\n");

        let resolve = Cli::try_parse_from(["cordy", "issue", "comment", "resolve", "comment-1"])
            .expect("resolve CLI");
        let output = run_with_input(&resolve, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("resolve comment");
        assert_eq!(output.stderr, "Comment comment-1 resolved.\n");
        assert!(
            serde_json::from_str::<Value>(&output.stdout).expect("resolved JSON")["resolved_at"]
                .is_string()
        );

        let unresolve = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "unresolve",
            "comment-1",
            "--output",
            "table",
        ])
        .expect("unresolve CLI");
        let output = run_with_input(&unresolve, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("unresolve comment");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "Comment comment-1 unresolved.\n");
        task.abort();
    }

    #[tokio::test]
    async fn issue_comment_list_parser_and_validation_match_go() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "list",
            "CORD-18",
            "--thread",
            "comment-1",
            "--tail",
            "0",
            "--summary",
            "--compact",
            "--full",
            "--before",
            "2026-08-24T00:00:00Z",
            "--before-id",
            "comment-2",
            "--output",
            "json",
        ])
        .expect("comment list CLI");
        let args = issue_comment_list_args(&cli);
        assert_eq!(args.thread.as_deref(), Some("comment-1"));
        assert_eq!(args.tail, Some(0));
        assert!(args.summary && args.compact && args.full);
        assert_eq!(args.output, OutputFormat::Json);

        let invalid = Cli::try_parse_from([
            "cordy", "issue", "comment", "list", "CORD-18", "--tail", "1",
        ])
        .expect("combination validation is at runtime");
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("tail requires thread");
        assert!(error.to_string().contains("--tail requires --thread"));
    }

    #[tokio::test]
    async fn issue_comment_list_sends_folded_recent_query_surfaces_cursor_and_compacts_json() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/comments",
                get(|request: Request| async move {
                    let query = request.uri().query().unwrap_or_default();
                    assert!(query.contains("summary=true"));
                    assert!(query.contains("fold=true"));
                    assert!(query.contains("recent=2"));
                    assert!(query.contains("before=2026-08-24T00%3A00%3A00Z"));
                    assert!(query.contains("before_id=comment-2"));
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "X-Cordy-Next-Before",
                        "2026-08-23T23:00:00Z".parse().expect("cursor"),
                    );
                    headers.insert(
                        "X-Cordy-Next-Before-Id",
                        "comment-older".parse().expect("cursor id"),
                    );
                    (
                        headers,
                        Json(vec![serde_json::json!({
                            "id":"comment-1","issue_id":"issue-uuid","source_task_id":null,
                            "author_type":"member","author_id":"member-1","type":"comment",
                            "content":"summary","created_at":"2026-08-24T00:00:00Z",
                            "updated_at":"2026-08-24T00:00:00Z","parent_id":null,
                            "attachments":[]
                        })]),
                    )
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "comment",
            "list",
            "CORD-18",
            "--recent",
            "2",
            "--summary",
            "--compact",
            "--before",
            "2026-08-24T00:00:00Z",
            "--before-id",
            "comment-2",
            "--output",
            "json",
        ])
        .expect("comment list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list comments");
        assert_eq!(
            output.stderr,
            "Next thread cursor: --before 2026-08-23T23:00:00Z --before-id comment-older\n"
        );
        let comments: Value = serde_json::from_str(&output.stdout).expect("comments JSON");
        let comment = &comments[0];
        assert!(comment.get("issue_id").is_none());
        assert!(comment.get("source_task_id").is_none());
        assert!(comment.get("updated_at").is_none());
        assert!(comment.get("parent_id").is_none());
        assert!(comment.get("attachments").is_none());
        task.abort();
    }

    #[test]
    fn issue_comment_list_table_truncates_and_formats_actor_fallback() {
        let comments = vec![serde_json::json!({
            "id":"comment-1","parent_id":null,"author_type":"agent","author_id":"agent-1",
            "type":"comment","content":"x".repeat(81),"created_at":"2026-08-24T12:34:56Z"
        })];
        let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CodeBot".into())]));
        let table = format_issue_comments_table(&comments, &actors);
        assert!(table.starts_with("ID"));
        assert!(table.contains("agent:CodeBot"));
        assert!(table.contains("2026-08-24T12:34"));
        assert!(table.contains("xxx..."));
    }

    #[test]
    fn issue_runs_parser_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "runs",
            "CORD-18",
            "--full-id",
            "--output",
            "json",
        ])
        .expect("runs CLI");
        let args = issue_runs_args(&cli);
        assert_eq!(args.issue_id, "CORD-18");
        assert!(args.full_id);
        assert_eq!(args.output, OutputFormat::Json);

        let runs = vec![serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","agent_id":"agent-1",
            "status":"failed","started_at":"2026-08-24T12:34:56Z",
            "completed_at":"2026-08-24T12:40:00Z","error":"x".repeat(51)
        })];
        let actors = IssueActorNames(HashMap::from([("agent:agent-1".into(), "CodeBot".into())]));
        let short = format_issue_runs_table(&runs, false, &actors);
        assert!(short.contains("11111111"));
        assert!(!short.contains("11111111-1111"));
        assert!(short.contains("CodeBot"));
        assert!(short.contains("2026-08-24T12:34"));
        assert!(short.contains("xxx..."));
        let full = format_issue_runs_table(&runs, true, &actors);
        assert!(full.contains("11111111-1111-1111-1111-111111111111"));
    }

    #[tokio::test]
    async fn issue_runs_resolves_issue_fetches_task_runs_and_actor_names() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/task-runs",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "id":"task-uuid","agent_id":"agent-1","status":"completed",
                        "started_at":"2026-08-24T12:34:56Z","completed_at":"2026-08-24T12:40:00Z"
                    })])
                }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"CodeBot"})]) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "runs", "CORD-18"]).expect("runs CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list runs");
        assert!(output.stdout.starts_with("ID"));
        assert!(output.stdout.contains("CodeBot"));
        assert!(output.stdout.contains("completed"));
        task.abort();
    }

    #[test]
    fn issue_run_controls_parser_and_message_table_match_go_contract() {
        let messages = Cli::try_parse_from([
            "cordy",
            "issue",
            "run-messages",
            "abcd",
            "--issue",
            "CORD-18",
            "--since",
            "4",
            "--output",
            "table",
        ])
        .expect("run-messages CLI");
        let args = issue_run_messages_args(&messages);
        assert_eq!(args.task_id, "abcd");
        assert_eq!(args.issue.as_deref(), Some("CORD-18"));
        assert_eq!(args.since, 4);
        assert_eq!(args.output, OutputFormat::Table);

        let cancel = Cli::try_parse_from([
            "cordy",
            "issue",
            "cancel-task",
            "11111111-1111-1111-1111-111111111111",
            "--output",
            "json",
        ])
        .expect("cancel-task CLI");
        assert_eq!(
            issue_cancel_task_args(&cancel).task_id,
            "11111111-1111-1111-1111-111111111111"
        );

        let table = format_issue_run_messages_table(&[
            serde_json::json!({
                "seq":1,"type":"text","tool":"","content":"done"
            }),
            serde_json::json!({
                "seq":2,"type":"tool_result","tool":"shell","content":"",
                "output":"x".repeat(81)
            }),
        ]);
        assert!(table.starts_with("SEQ"));
        assert!(table.contains("done"));
        assert!(table.contains("tool_result"));
        assert!(table.contains("xxx..."));
    }

    #[tokio::test]
    async fn issue_run_messages_resolves_scoped_prefix_and_sends_since() {
        let issue_id = "1881a167-4bb6-4602-944b-f40ce4192fe6";
        let task_id = "abcd1234-0000-0000-0000-000000000000";
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || async move {
                    Json(serde_json::json!({"id":issue_id,"identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/1881a167-4bb6-4602-944b-f40ce4192fe6/task-runs",
                get(move || async move { Json(vec![serde_json::json!({"id":task_id})]) }),
            )
            .route(
                "/api/tasks/abcd1234-0000-0000-0000-000000000000/messages",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("since=4"));
                    Json(vec![serde_json::json!({
                        "seq":5,"type":"text","content":"done"
                    })])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "run-messages",
            "abcd",
            "--issue",
            "CORD-18",
            "--since",
            "4",
        ])
        .expect("run-messages CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("run messages");
        let messages: Value = serde_json::from_str(&output.stdout).expect("messages JSON");
        assert_eq!(messages[0]["seq"], 5);
        task.abort();
    }

    #[tokio::test]
    async fn issue_cancel_task_posts_empty_body_and_requires_scope_for_prefix() {
        let task_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new().route(
            "/api/tasks/11111111-1111-1111-1111-111111111111/cancel",
            post(move |Json(body): Json<Value>| async move {
                assert_eq!(body, serde_json::json!({}));
                Json(serde_json::json!({"id":task_id,"status":"cancelled"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "cancel-task",
            task_id,
            "--output",
            "table",
        ])
        .expect("cancel-task CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("cancel task");
        assert_eq!(
            output.stdout,
            "Task 11111111-1111-1111-1111-111111111111 -> status=cancelled\n"
        );

        let missing_scope = Cli::try_parse_from(["cordy", "issue", "cancel-task", "abcd"])
            .expect("short cancel CLI");
        let error = run_with_input(
            &missing_scope,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect_err("short task prefix requires issue");
        assert!(error.to_string().contains("require --issue"));
        task.abort();
    }

    #[test]
    fn issue_usage_parser_and_number_format_match_go() {
        let cli = Cli::try_parse_from(["cordy", "issue", "usage", "CORD-18", "--output", "json"])
            .expect("usage CLI");
        let args = issue_usage_args(&cli);
        assert_eq!(args.issue_id, "CORD-18");
        assert_eq!(args.output, OutputFormat::Json);
        assert_eq!(format_metadata_value(Some(&serde_json::json!(42.0))), "42");
        assert_eq!(
            format_metadata_value(Some(&serde_json::json!(1234567890123_u64))),
            "1234567890123"
        );
        assert_eq!(format_metadata_value(None), "null");
    }

    #[tokio::test]
    async fn issue_usage_resolves_issue_and_renders_aggregate_table() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/usage",
                get(|| async {
                    Json(serde_json::json!({
                        "total_input_tokens":1000,"total_output_tokens":200,
                        "total_cache_read_tokens":300,"total_cache_write_tokens":40,"task_count":2
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "usage", "CORD-18"]).expect("usage CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("issue usage");
        assert!(output.stdout.starts_with("INPUT_TOKENS"));
        assert!(output.stdout.contains("1000"));
        assert!(output.stdout.contains("300"));
        assert!(output.stdout.contains("2"));
        task.abort();
    }

    #[tokio::test]
    async fn issue_rerun_posts_fresh_task_and_formats_agent_name() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/rerun",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({}));
                    Json(serde_json::json!({"id":"task-1","agent_id":"agent-1","status":"queued"}))
                }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"CodeBot"})]) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "issue", "rerun", "CORD-18", "--output", "table"])
            .expect("rerun CLI");
        assert_eq!(issue_rerun_args(&cli).issue_id, "CORD-18");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("rerun issue");
        assert_eq!(output.stdout, "Re-enqueued task task-1 on agent CodeBot\n");
        assert!(output.stderr.is_empty());
        task.abort();
    }

    #[test]
    fn issue_search_parser_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "search",
            "cache bug",
            "--limit",
            "5",
            "--include-closed",
            "--output",
            "json",
        ])
        .expect("search CLI");
        let args = issue_search_args(&cli);
        assert_eq!(args.query, "cache bug");
        assert_eq!(args.limit, 5);
        assert!(args.include_closed);
        assert_eq!(args.output, OutputFormat::Json);

        let table = format_issue_search_table(&[serde_json::json!({
            "identifier":"CORD-18","title":"Cache issue","status":"todo",
            "match_source":"comment","matched_snippet":"x".repeat(51)
        })]);
        assert!(table.starts_with("KEY"));
        assert!(table.contains("CORD-18"));
        assert!(table.contains("comment: "));
        assert!(table.contains("xxx..."));
    }

    #[tokio::test]
    async fn issue_search_encodes_query_and_preserves_json_envelope() {
        let app = Router::new().route(
            "/api/issues/search",
            get(|request: Request| async move {
                let query = request.uri().query().unwrap_or_default();
                assert!(query.contains("q=cache+bug"));
                assert!(query.contains("limit=5"));
                assert!(query.contains("include_closed=true"));
                Json(serde_json::json!({
                    "issues":[{"id":"issue-1","identifier":"CORD-18","title":"Cache bug"}],
                    "total":1
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "search",
            "cache bug",
            "--limit",
            "5",
            "--include-closed",
            "--output",
            "json",
        ])
        .expect("search CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("search issues");
        let result: Value = serde_json::from_str(&output.stdout).expect("search JSON");
        assert_eq!(result["total"], 1);
        assert_eq!(result["issues"][0]["identifier"], "CORD-18");
        task.abort();
    }

    #[test]
    fn issue_subscriber_parser_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "subscriber",
            "add",
            "CORD-18",
            "--user-id",
            "11111111-1111-1111-1111-111111111111",
            "--output",
            "table",
        ])
        .expect("subscriber add CLI");
        let Command::Issue(IssueArgs {
            command:
                IssueCommand::Subscriber(IssueSubscriberArgs {
                    command: IssueSubscriberCommand::Add(args),
                }),
        }) = &cli.command
        else {
            panic!("expected subscriber add");
        };
        assert_eq!(args.issue_id, "CORD-18");
        assert_eq!(
            args.user_id.as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(args.output, OutputFormat::Table);

        let subscribers = [serde_json::json!({
            "user_type":"member","user_id":"member-1","reason":"manual",
            "created_at":"2026-08-24T12:34:56Z"
        })];
        let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
        let table = format_issue_subscribers_table(&subscribers, &actors);
        assert!(table.starts_with("USER"));
        assert!(table.contains("member:Ada"));
        assert!(table.contains("manual"));
        assert!(table.contains("2026-08-24T12:34"));
    }

    #[tokio::test]
    async fn issue_subscriber_list_resolves_issue_and_preserves_json() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/subscribers",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "user_type":"agent","user_id":"agent-1","reason":"mentioned",
                        "created_at":"2026-08-24T12:34:56Z"
                    })])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "subscriber",
            "list",
            "CORD-18",
            "--output",
            "json",
        ])
        .expect("subscriber list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list subscribers");
        let subscribers: Value = serde_json::from_str(&output.stdout).expect("subscribers JSON");
        assert_eq!(subscribers[0]["user_id"], "agent-1");
        assert!(output.stderr.is_empty());
        task.abort();
    }

    #[tokio::test]
    async fn issue_subscriber_mutation_defaults_to_caller_and_resolves_members_only() {
        let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
        let subscribe_bodies = Arc::clone(&bodies);
        let unsubscribe_bodies = Arc::clone(&bodies);
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/subscribe",
                post(move |Json(body): Json<Value>| {
                    let bodies = Arc::clone(&subscribe_bodies);
                    async move {
                        bodies.lock().expect("bodies").push(body);
                        Json(serde_json::json!({"subscribed":true}))
                    }
                }),
            )
            .route(
                "/api/issues/issue-uuid/unsubscribe",
                post(move |Json(body): Json<Value>| {
                    let bodies = Arc::clone(&unsubscribe_bodies);
                    async move {
                        bodies.lock().expect("bodies").push(body);
                        Json(serde_json::json!({"subscribed":false}))
                    }
                }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "user_id":"11111111-1111-1111-1111-111111111111","name":"Ada",
                        "email":"ada@example.com"
                    })])
                }),
            )
            .route("/api/agents", get(|| async { Json(Vec::<Value>::new()) }));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let caller = Cli::try_parse_from(["cordy", "issue", "subscriber", "add", "CORD-18"])
            .expect("subscriber caller CLI");
        let caller_output =
            run_with_input(&caller, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("subscribe caller");
        assert_eq!(
            caller_output.stderr,
            "Subscribed caller to issue CORD-18.\n"
        );

        let member = Cli::try_parse_from([
            "cordy",
            "issue",
            "subscriber",
            "remove",
            "CORD-18",
            "--user-id",
            "11111111-1111-1111-1111-111111111111",
            "--output",
            "table",
        ])
        .expect("subscriber member CLI");
        let member_output =
            run_with_input(&member, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("unsubscribe member");
        assert!(member_output.stdout.is_empty());
        assert_eq!(
            member_output.stderr,
            "Unsubscribed member:Ada to issue CORD-18.\n"
        );
        assert_eq!(
            *bodies.lock().expect("bodies"),
            vec![
                serde_json::json!({}),
                serde_json::json!({
                    "user_type":"member",
                    "user_id":"11111111-1111-1111-1111-111111111111"
                })
            ]
        );
        task.abort();
    }

    #[test]
    fn issue_label_parser_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "label",
            "add",
            "CORD-18",
            "abcd",
            "--full-id",
            "--output",
            "json",
        ])
        .expect("issue label add CLI");
        let Command::Issue(IssueArgs {
            command:
                IssueCommand::Label(IssueLabelArgs {
                    command: IssueLabelCommand::Add(args),
                }),
        }) = &cli.command
        else {
            panic!("expected issue label add");
        };
        assert_eq!(args.issue_id, "CORD-18");
        assert_eq!(args.label_id, "abcd");
        assert!(args.full_id);
        assert_eq!(args.output, OutputFormat::Json);

        let labels = [serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","name":"Bug","color":"#ff0000"
        })];
        let short = format_label_table(&labels, false);
        assert!(short.starts_with("ID"));
        assert!(short.contains("11111111"));
        assert!(!short.contains("11111111-1111"));
        assert!(short.contains("Bug"));
        let full = format_label_table(&labels, true);
        assert!(full.contains("11111111-1111-1111-1111-111111111111"));
    }

    #[tokio::test]
    async fn issue_label_add_resolves_prefix_and_returns_response_labels() {
        let label_id = "abcd1234-0000-0000-0000-000000000000";
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/labels",
                get(move |request: Request| async move {
                    assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                    Json(serde_json::json!({
                        "labels":[{"id":label_id,"name":"Bug","color":"#ff0000"}]
                    }))
                }),
            )
            .route(
                "/api/issues/issue-uuid/labels",
                post(move |Json(body): Json<Value>| async move {
                    assert_eq!(body["label_id"], label_id);
                    Json(serde_json::json!({
                        "labels":[{"id":label_id,"name":"Bug","color":"#ff0000"}]
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "label", "add", "CORD-18", "abcd", "--output", "json",
        ])
        .expect("issue label add CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("attach label");
        let labels: Value = serde_json::from_str(&output.stdout).expect("labels JSON");
        assert_eq!(labels[0]["name"], "Bug");
        task.abort();
    }

    #[tokio::test]
    async fn issue_label_remove_preserves_success_when_refresh_fails() {
        let issue_id = "11111111-1111-1111-1111-111111111111";
        let label_id = "22222222-2222-2222-2222-222222222222";
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(move || async move {
                    Json(serde_json::json!({"id":issue_id,"identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/labels/22222222-2222-2222-2222-222222222222",
                delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
            )
            .route(
                "/api/issues/11111111-1111-1111-1111-111111111111/labels",
                get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "label", "remove", "CORD-18", label_id, "--output", "json",
        ])
        .expect("issue label remove CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("detach label");
        assert_eq!(
            serde_json::from_str::<Value>(&output.stdout).expect("detach JSON"),
            serde_json::json!({"detached":true})
        );
        task.abort();
    }

    #[test]
    fn label_parser_and_tables_match_go_registry_contract() {
        let create = Cli::try_parse_from([
            "cordy", "label", "create", "--name", "Bug", "--color", "#ff0000", "--output", "table",
        ])
        .expect("label create CLI");
        let Command::Label(LabelArgs {
            command: LabelCommand::Create(args),
        }) = &create.command
        else {
            panic!("expected label create");
        };
        assert_eq!(args.name.as_deref(), Some("Bug"));
        assert_eq!(args.color.as_deref(), Some("#ff0000"));
        assert_eq!(args.output, OutputFormat::Table);

        let label = serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","name":"Bug","color":"#ff0000",
            "created_at":"2026-08-24T12:34:56Z"
        });
        let short = format_workspace_label_table(std::slice::from_ref(&label), false);
        assert!(short.starts_with("ID"));
        assert!(short.contains("11111111"));
        assert!(short.contains("2026-08-24"));
        let details = format_label_result(&label, OutputFormat::Table, true).expect("details");
        assert!(details.contains("11111111-1111-1111-1111-111111111111"));
    }

    #[tokio::test]
    async fn label_create_update_and_delete_use_go_http_and_output_contracts() {
        let label_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new()
            .route(
                "/api/labels",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"name":"Bug","color":"#ff0000"}));
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Bug","color":"#ff0000"
                    }))
                }),
            )
            .route(
                "/api/labels/11111111-1111-1111-1111-111111111111",
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"name":"Defect"}));
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Defect","color":"#ff0000"
                    }))
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let create = Cli::try_parse_from([
            "cordy", "label", "create", "--name", "Bug", "--color", "#ff0000",
        ])
        .expect("label create CLI");
        let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create label");
        assert_eq!(
            serde_json::from_str::<Value>(&created.stdout).expect("created JSON")["name"],
            "Bug"
        );

        let update = Cli::try_parse_from([
            "cordy", "label", "update", label_id, "--name", "Defect", "--output", "table",
        ])
        .expect("label update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update label");
        assert!(updated.stdout.contains("Defect"));

        let delete =
            Cli::try_parse_from(["cordy", "label", "delete", label_id, "--output", "json"])
                .expect("label delete CLI");
        let deleted = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete label");
        let deleted: Value = serde_json::from_str(&deleted.stdout).expect("deleted JSON");
        assert_eq!(deleted["id"], label_id);
        assert_eq!(deleted["deleted"], true);
        task.abort();
    }

    #[test]
    fn issue_metadata_parser_value_types_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy", "issue", "metadata", "set", "CORD-18", "--key", "attempt", "--value=",
            "--type", "string", "--output", "json",
        ])
        .expect("metadata set CLI");
        let Command::Issue(IssueArgs {
            command:
                IssueCommand::Metadata(IssueMetadataArgs {
                    command: IssueMetadataCommand::Set(args),
                }),
        }) = &cli.command
        else {
            panic!("expected metadata set");
        };
        assert_eq!(args.key.as_deref(), Some("attempt"));
        assert_eq!(args.value.as_deref(), Some(""));
        assert_eq!(args.value_type.as_deref(), Some("string"));
        assert_eq!(
            parse_metadata_value("true", None).expect("bool"),
            Value::Bool(true)
        );
        assert_eq!(
            parse_metadata_value("3.5", None).expect("number"),
            serde_json::json!(3.5)
        );
        assert_eq!(
            parse_metadata_value("42", Some("string")).expect("forced string"),
            Value::String("42".into())
        );
        assert!(parse_metadata_value("yes", Some("bool"))
            .expect_err("invalid bool")
            .to_string()
            .contains("expected true or false"));

        let metadata = serde_json::Map::from_iter([
            ("zeta".into(), serde_json::json!(2)),
            ("alpha".into(), serde_json::json!(true)),
        ]);
        let table = format_metadata_table(&metadata);
        assert!(table.starts_with("KEY"));
        assert!(table.find("alpha").expect("alpha") < table.find("zeta").expect("zeta"));
        assert!(table.contains("bool"));
        assert!(table.contains("number"));
    }

    #[tokio::test]
    async fn issue_metadata_list_degrades_only_not_found_to_empty() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/metadata",
                get(|| async { axum::http::StatusCode::NOT_FOUND }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "metadata", "list", "CORD-18", "--output", "json",
        ])
        .expect("metadata list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("metadata list fallback");
        assert_eq!(
            serde_json::from_str::<Value>(&output.stdout).expect("metadata JSON"),
            serde_json::json!({})
        );
        task.abort();
    }

    #[tokio::test]
    async fn issue_metadata_set_puts_typed_value_and_returns_full_map() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/metadata/attempt",
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"value":3}));
                    Json(serde_json::json!({"metadata":{"attempt":3,"ready":true}}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "metadata", "set", "CORD-18", "--key", "attempt", "--value", "3",
            "--type", "number", "--output", "json",
        ])
        .expect("metadata set CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("set metadata");
        let metadata: Value = serde_json::from_str(&output.stdout).expect("metadata JSON");
        assert_eq!(metadata["attempt"], 3);
        assert_eq!(metadata["ready"], true);
        task.abort();
    }

    #[test]
    fn issue_timeline_parser_filter_and_table_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "history",
            "CORD-18",
            "--action",
            "status_changed,priority_changed",
            "--since",
            "2026-08-19T00:00:00Z",
            "--tail",
            "1",
            "--full-id",
        ])
        .expect("timeline CLI alias");
        let Command::Issue(IssueArgs {
            command: IssueCommand::Timeline(args),
        }) = &cli.command
        else {
            panic!("expected issue timeline");
        };
        let filter = build_timeline_filter(args).expect("timeline filter");
        assert!(filter.activity_only);
        assert!(filter.actions.contains("status_changed"));
        assert_eq!(filter.tail, 1);
        let entries = filter_timeline(
            vec![
                serde_json::json!({
                    "type":"comment","created_at":"2026-08-20T00:00:00Z","content":"ignored"
                }),
                serde_json::json!({
                    "type":"activity","action":"status_changed",
                    "created_at":"2026-08-20T00:00:00Z","details":{"from":"todo","to":"done"}
                }),
                serde_json::json!({
                    "type":"activity","action":"priority_changed",
                    "created_at":"2026-08-21T00:00:00Z","details":{"from":"low","to":"high"}
                }),
            ],
            &filter,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(value_string(&entries[0], "action"), "priority_changed");

        let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
        let table = format_issue_timeline_table(
            &[
                serde_json::json!({
                    "type":"activity","action":"assignee_changed",
                    "actor_type":"member","actor_id":"member-1",
                    "created_at":"2026-08-24T12:34:56Z",
                    "details":{"from_type":"member","from_id":"old-member","to_type":"member","to_id":"member-1"}
                }),
                serde_json::json!({
                    "type":"comment","actor_type":"system","actor_id":null,
                    "created_at":"2026-08-24T13:00:00Z",
                    "content":"multi\nline   comment"
                }),
            ],
            &actors,
            false,
        );
        assert!(table.starts_with("TIME"));
        assert!(table.contains("member:Ada"));
        assert!(table.contains("member:old-memb → member:Ada"));
        assert!(table.contains("multi line comment"));
        assert!(table.contains("system"));
    }

    #[test]
    fn issue_timeline_rejects_invalid_since_and_negative_tail() {
        let invalid_since = Cli::try_parse_from([
            "cordy",
            "issue",
            "timeline",
            "CORD-18",
            "--since",
            "yesterday",
        ])
        .expect("invalid since parses");
        let Command::Issue(IssueArgs {
            command: IssueCommand::Timeline(args),
        }) = &invalid_since.command
        else {
            panic!("expected timeline");
        };
        assert!(build_timeline_filter(args)
            .expect_err("invalid since")
            .to_string()
            .contains("expected RFC3339"));

        let negative_tail =
            Cli::try_parse_from(["cordy", "issue", "timeline", "CORD-18", "--tail", "-1"])
                .expect("negative tail parses");
        let Command::Issue(IssueArgs {
            command: IssueCommand::Timeline(args),
        }) = &negative_tail.command
        else {
            panic!("expected timeline");
        };
        assert_eq!(
            build_timeline_filter(args)
                .expect_err("negative tail")
                .to_string(),
            "--tail must be >= 0"
        );
    }

    #[tokio::test]
    async fn issue_timeline_filters_json_and_surfaces_truncation_header() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/issues/issue-uuid/timeline",
                get(|| async {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "X-Timeline-Truncated",
                        "activity,comment".parse().expect("truncation header"),
                    );
                    (
                        headers,
                        Json(vec![
                            serde_json::json!({
                                "type":"comment","created_at":"2026-08-20T00:00:00Z","content":"note"
                            }),
                            serde_json::json!({
                                "type":"activity","action":"status_changed",
                                "created_at":"2026-08-21T00:00:00Z","details":{"from":"todo","to":"done"}
                            }),
                        ]),
                    )
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "timeline",
            "CORD-18",
            "--activity-only",
            "--output",
            "json",
        ])
        .expect("timeline CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("timeline");
        let entries: Value = serde_json::from_str(&output.stdout).expect("timeline JSON");
        assert_eq!(entries.as_array().expect("entries").len(), 1);
        assert_eq!(entries[0]["action"], "status_changed");
        assert!(output.stderr.contains("activity,comment"));
        assert!(output.stderr.contains("older entries are missing"));
        task.abort();
    }

    #[test]
    fn issue_property_parser_resolution_and_rendering_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "issue",
            "property",
            "set",
            "CORD-18",
            "--name",
            "Platforms",
            "--value=",
            "--output",
            "json",
        ])
        .expect("property set CLI");
        let Command::Issue(IssueArgs {
            command:
                IssueCommand::Property(IssuePropertyArgs {
                    command: IssuePropertyCommand::Set(args),
                }),
        }) = &cli.command
        else {
            panic!("expected issue property set");
        };
        assert_eq!(args.value.as_deref(), Some(""));

        let definitions: Vec<PropertyDefinition> = serde_json::from_value(serde_json::json!([
            {
                "id":"property-1","name":"Severity","type":"select","archived":false,
                "config":{"options":[{"id":"option-1","name":"Critical","color":"#f00"}]}
            },
            {
                "id":"property-2","name":"Reviewer","type":"actor","archived":true,
                "config":{"options":[]}
            }
        ]))
        .expect("property definitions");
        assert_eq!(
            resolve_property(&definitions, "severity")
                .expect("case-insensitive name")
                .id,
            "property-1"
        );
        let bag = serde_json::Map::from_iter([
            ("property-1".into(), Value::String("option-1".into())),
            ("property-2".into(), Value::String("member:member-1".into())),
        ]);
        let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
        let rows = build_issue_property_rows(&definitions, &bag, &actors);
        assert_eq!(rows[0].display, "Critical");
        assert_eq!(rows[1].display, "Ada");
        let table = format_issue_property_rows(&rows, OutputFormat::Table).expect("table");
        assert!(table.starts_with("NAME"));
        assert!(table.contains("Severity"));
        assert!(table.contains("Reviewer"));
        let json = format_issue_property_rows(&rows, OutputFormat::Json).expect("JSON");
        let json: Value = serde_json::from_str(&json).expect("rows JSON");
        assert!(json[0].get("archived").is_none());
        assert_eq!(json[1]["archived"], true);
    }

    #[tokio::test]
    async fn issue_property_set_resolves_option_name_and_puts_typed_value() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/properties",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("include_archived=true"));
                    Json(serde_json::json!({
                        "properties":[{
                            "id":"property-1","name":"Severity","type":"select",
                            "config":{"options":[{"id":"option-1","name":"Critical","color":"#f00"}]}
                        }]
                    }))
                }),
            )
            .route(
                "/api/issues/issue-uuid/properties/property-1",
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"value":"option-1"}));
                    Json(serde_json::json!({"properties":{"property-1":"option-1"}}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "property", "set", "CORD-18", "--name", "severity", "--value",
            "Critical", "--output", "json",
        ])
        .expect("property set CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("set issue property");
        let rows: Value = serde_json::from_str(&output.stdout).expect("property rows JSON");
        assert_eq!(rows[0]["display"], "Critical");
        assert_eq!(rows[0]["value"], "option-1");
        task.abort();
    }

    #[tokio::test]
    async fn issue_property_list_resolves_member_actor_display() {
        let app = Router::new()
            .route(
                "/api/issues/CORD-18",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","identifier":"CORD-18"}))
                }),
            )
            .route(
                "/api/properties",
                get(|| async {
                    Json(serde_json::json!({
                        "properties":[{
                            "id":"property-1","name":"Reviewer","type":"actor","config":{}
                        }]
                    }))
                }),
            )
            .route(
                "/api/issues/issue-uuid",
                get(|| async {
                    Json(serde_json::json!({"id":"issue-uuid","properties":{"property-1":"member:member-1"}}))
                }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async {
                    Json(vec![serde_json::json!({"user_id":"member-1","name":"Ada"})])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy", "issue", "property", "list", "CORD-18", "--output", "table",
        ])
        .expect("property list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list issue properties");
        assert!(output.stdout.contains("Reviewer"));
        assert!(output.stdout.contains("Ada"));
        task.abort();
    }

    #[test]
    fn config_agent_timeout_display_preserves_three_states() {
        let path = Path::new("/tmp/config.json");

        let disabled =
            format_config_table(path, "", &[("agent_timeout", Value::String("0s".into()))]);
        assert!(disabled.contains("0s (disabled)"));

        let positive =
            format_config_table(path, "", &[("agent_timeout", Value::String("30m".into()))]);
        assert!(positive.contains("30m"));
        assert!(!positive.contains("disabled"));

        let unset = format_config_table(path, "", &[("agent_timeout", Value::Null)]);
        assert!(unset.contains("(not set)"));
    }

    #[tokio::test]
    async fn config_show_table_and_json_exclude_credentials_and_unknown_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_path = home.path().join(".cordy/profiles/dev/config.json");
        fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
        fs::write(
            &profile_path,
            r#"{
  "server_url": "https://api.example.com",
  "workspace_id": "workspace-1",
  "agent_timeout": "0s",
  "disable_auto_update": true,
  "token": "mul_secret",
  "future_secret": "do-not-print"
}"#,
        )
        .expect("profile config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let table = Cli::try_parse_from(["cordy", "--profile", "dev", "config"])
            .expect("config default-show CLI");
        let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("config table");
        assert!(output.stdout.contains("Profile:      dev"));
        assert!(output.stdout.contains("agent_timeout:"));
        assert!(output.stdout.contains("0s (disabled)"));
        assert!(output.stdout.contains("disable_auto_update:"));
        assert!(!output.stdout.contains("mul_secret"));
        assert!(!output.stdout.contains("do-not-print"));

        let json = Cli::try_parse_from([
            "cordy",
            "--profile",
            "dev",
            "config",
            "show",
            "--output",
            "json",
        ])
        .expect("config JSON CLI");
        let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("config JSON");
        let config: Value = serde_json::from_str(&output.stdout).expect("config JSON output");
        assert_eq!(config["profile"], "dev");
        assert_eq!(config["server_url"], "https://api.example.com");
        assert_eq!(config["disable_auto_update"], true);
        assert!(config.get("token").is_none());
        assert!(config.get("future_secret").is_none());
    }

    #[tokio::test]
    async fn config_set_is_profile_scoped_and_preserves_unrelated_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let default_path = home.path().join(".cordy/config.json");
        let profile_path = home.path().join(".cordy/profiles/dev/config.json");
        fs::create_dir_all(default_path.parent().expect("default parent")).expect("default dir");
        fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
        let default_bytes = br#"{"server_url":"https://default.example","token":"mul_default"}"#;
        fs::write(&default_path, default_bytes).expect("default config");
        fs::write(
            &profile_path,
            r#"{"token":"mul_dev","future":{"keep":true}}"#,
        )
        .expect("profile config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        for (key, value, expected) in [
            (
                "server_url",
                "https://api.dev.example",
                "https://api.dev.example",
            ),
            ("heartbeat_interval", " 5s ", "5s"),
            ("max_concurrent_tasks", "4", "4"),
            ("disable_auto_reload", "true", "true"),
        ] {
            let cli =
                Cli::try_parse_from(["cordy", "--profile", "dev", "config", "set", key, value])
                    .expect("config set CLI");
            let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("config set");
            assert_eq!(output.stderr, format!("Set {key} = {expected}\n"));
        }
        let saved: Value = serde_json::from_slice(&fs::read(&profile_path).expect("saved profile"))
            .expect("saved JSON");
        assert_eq!(saved["token"], "mul_dev");
        assert_eq!(saved["future"]["keep"], true);
        assert_eq!(saved["heartbeat_interval"], "5s");
        assert_eq!(saved["max_concurrent_tasks"], 4);
        assert_eq!(saved["disable_auto_reload"], true);
        assert_eq!(
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
    }

    #[test]
    fn config_set_whitelist_and_validation_match_registry_contract() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let root = cwd.path().join("data/cordy").display().to_string();
        let valid = [
            ("server_url", "https://api.example.com"),
            ("app_url", "https://app.example.com"),
            ("workspace_id", "workspace-1"),
            ("device_name", "host-a"),
            ("runtime_name", "runtime-a"),
            ("workspaces_root", "data/cordy"),
            ("max_concurrent_tasks", "8"),
            ("poll_interval", "1m30s"),
            ("heartbeat_interval", " 5s "),
            ("agent_timeout", "0s"),
            ("codex_semantic_inactivity_timeout", "15m"),
            ("codex_handshake_timeout", "45s"),
            ("disable_auto_update", "TRUE"),
            ("auto_update_check_interval", "12h"),
            ("disable_auto_reload", "false"),
        ];
        for (key, value) in valid {
            let (_, displayed) =
                validate_config_set(key, value, &environment).expect("valid config value");
            if key == "workspaces_root" {
                assert_eq!(displayed, root);
            }
        }
        for (key, value, message) in [
            ("token", "secret", "unknown config key"),
            ("server_url", "not a URL", "valid URL"),
            ("app_url", "ftp://example.com", "must use one of"),
            ("max_concurrent_tasks", "-1", ">= 0"),
            ("poll_interval", "0s", "positive"),
            ("heartbeat_interval", "abc", "duration"),
            ("agent_timeout", "-1s", ">= 0"),
            ("disable_auto_update", "maybe", "true"),
        ] {
            assert!(validate_config_set(key, value, &environment)
                .expect_err("invalid config value")
                .to_string()
                .contains(message));
        }
    }

    #[tokio::test]
    async fn config_commands_fail_closed_without_task_local_root() {
        let home = tempfile::tempdir().expect("owner home");
        let cwd = tempfile::tempdir().expect("task cwd");
        let owner_path = home.path().join(".cordy/config.json");
        fs::create_dir_all(owner_path.parent().expect("owner parent")).expect("owner dir");
        let owner_bytes = br#"{"server_url":"https://owner.invalid","token":"mul_owner"}"#;
        fs::write(&owner_path, owner_bytes).expect("owner config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_AGENT_ID", "agent-1");
        environment.set("CORDY_TASK_ID", "task-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "config",
            "set",
            "server_url",
            "https://task.example",
        ])
        .expect("task config set CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing task root");
        assert!(error.to_string().contains("task-local Cordy config root"));
        assert_eq!(fs::read(&owner_path).expect("owner unchanged"), owner_bytes);

        let task_root = tempfile::tempdir().expect("task root");
        environment.set(
            config::TASK_CONFIG_ROOT_ENV,
            task_root.path().display().to_string(),
        );
        run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("task-local config set");
        let task: Value = serde_json::from_slice(
            &fs::read(task_root.path().join("config.json")).expect("task config"),
        )
        .expect("task config JSON");
        assert_eq!(task["server_url"], "https://task.example");
        assert_eq!(
            fs::read(&owner_path).expect("owner still unchanged"),
            owner_bytes
        );
    }

    #[tokio::test]
    async fn auth_status_matches_human_table_and_json_contracts() {
        let app = Router::new().route(
            "/api/me",
            get(|request: Request| async move {
                assert_eq!(
                    request.headers()["authorization"],
                    "Bearer mul_env_status_token"
                );
                assert!(request.headers().get("x-workspace-id").is_none());
                assert!(request.headers().get("x-agent-id").is_none());
                Json(serde_json::json!({"name":"Ada","email":"ada@example.com"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "mul_env_status_token");

        let table = Cli::try_parse_from(["cordy", "auth", "status"]).expect("status CLI");
        let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("table status");
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            format!(
                "Server:  http://{address}\nUser:    Ada (ada@example.com)\nToken:   {}\n",
                display_token_prefix("mul_env_status_token")
            )
        );

        let json = Cli::try_parse_from(["cordy", "auth", "status", "--output", "json"])
            .expect("JSON status CLI");
        let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("JSON status");
        let status: Value = serde_json::from_str(&output.stdout).expect("status JSON");
        assert_eq!(status["authenticated"], true);
        assert_eq!(status["user"]["email"], "ada@example.com");
        assert_eq!(
            status["token"],
            display_token_prefix("mul_env_status_token")
        );
        server.abort();
    }

    #[tokio::test]
    async fn auth_status_task_context_requires_mat_token_and_never_prints_it() {
        let app = Router::new().route(
            "/api/me",
            get(|request: Request| async move {
                assert_eq!(
                    request.headers()["authorization"],
                    "Bearer mat_task_status_secret"
                );
                Json(serde_json::json!({"name":"Task Agent","email":"task@example.test"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let task_root = tempfile::tempdir().expect("task root");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_AGENT_ID", "agent-1");
        environment.set("CORDY_TASK_ID", "task-1");
        environment.set("CORDY_TOKEN", "mat_task_status_secret");
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        let cli = Cli::try_parse_from(["cordy", "auth", "status", "--output", "json"])
            .expect("task status CLI");
        let missing_root = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("task-local config root required");
        assert!(missing_root
            .to_string()
            .contains(config::TASK_CONFIG_ROOT_ENV));

        environment.set(
            config::TASK_CONFIG_ROOT_ENV,
            task_root.path().display().to_string(),
        );
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("task status");
        assert!(!output.stdout.contains("mat_task_status_secret"));
        assert!(serde_json::from_str::<Value>(&output.stdout)
            .expect("task status JSON")
            .get("token")
            .is_none());

        environment.set("CORDY_TOKEN", "mul_owner_token");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("human token rejected in task");
        assert!(error.to_string().contains("task-scoped mat_ token"));
        server.abort();
    }

    #[test]
    fn auth_logout_only_clears_current_profile_and_is_task_guarded() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let default_path = home.path().join(".cordy/config.json");
        let profile_path = home.path().join(".cordy/profiles/dev/config.json");
        fs::create_dir_all(default_path.parent().expect("default parent")).expect("default dir");
        fs::create_dir_all(profile_path.parent().expect("profile parent")).expect("profile dir");
        let default_bytes = br#"{"token":"mul_default","workspace_id":"default"}"#;
        fs::write(&default_path, default_bytes).expect("default config");
        fs::write(
            &profile_path,
            r#"{"token":"mul_dev","server_url":"https://dev.example","future":7}"#,
        )
        .expect("profile config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_TOKEN", "mul_env_must_not_affect_logout");
        let cli = Cli::try_parse_from(["cordy", "--profile", "dev", "auth", "logout"])
            .expect("logout CLI");
        let output = run_auth_logout(&cli, &environment).expect("logout");
        assert_eq!(output.stderr, "Token removed. You are now logged out.\n");
        let saved: Value = serde_json::from_slice(&fs::read(&profile_path).expect("saved profile"))
            .expect("profile JSON");
        assert!(saved.get("token").is_none());
        assert_eq!(saved["future"], 7);
        assert_eq!(
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
        assert_eq!(
            run_auth_logout(&cli, &environment)
                .expect("idempotent logout")
                .stderr,
            "Not authenticated.\n"
        );

        environment.set("CORDY_AGENT_ID", "agent-1");
        assert!(run_auth_logout(&cli, &environment)
            .expect_err("task logout rejected")
            .to_string()
            .contains("not available inside a daemon-managed task"));
    }

    #[tokio::test]
    async fn user_profile_get_is_a_real_configured_api_command() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let config_dir = home.path().join(".cordy");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("config.json"),
            r#"{"server_url":"http://127.0.0.1:1","token":"config-token","workspace_id":"config-workspace","future_field":true}"#,
        )
        .expect("config");
        let (server_url, server) = test_server().await;
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("{server_url}/ws?discard=yes"));
        environment.set("CORDY_TOKEN", "token-from-env");
        environment.set("CORDY_WORKSPACE_ID", "workspace-from-env");
        let cli = Cli::try_parse_from(["cordy", "user", "profile", "get", "--output", "json"])
            .expect("parse CLI");

        let output = run(&cli, &environment).await.expect("run profile get");
        let json: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(json["profile_description"], "Maintainer");
        server.abort();
    }

    #[tokio::test]
    async fn user_profile_update_patches_resolved_description() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let (server_url, captured, server) = patch_test_server().await;
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", server_url);
        environment.set("CORDY_TOKEN", "token-from-env");
        let cli = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description",
            r"Reviewer\nTypeScript",
            "--output",
            "json",
        ])
        .expect("parse CLI");
        let mut input = Cursor::new(Vec::<u8>::new());

        let output = run_with_input(&cli, &environment, &mut input)
            .await
            .expect("update profile");

        assert_eq!(
            captured
                .lock()
                .expect("captured body")
                .as_ref()
                .expect("body")["profile_description"],
            "Reviewer\nTypeScript"
        );
        let json: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(json["profile_description"], "Reviewer\nTypeScript");
        server.abort();
    }

    #[test]
    fn profile_update_text_sources_match_go_semantics() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let stdin_cli =
            Cli::try_parse_from(["cordy", "user", "profile", "update", "--description-stdin"])
                .expect("stdin CLI");
        let mut input = Cursor::new(b"first line\nsecond \\n literal\n".to_vec());
        assert_eq!(
            resolve_profile_description(update_args(&stdin_cli), &environment, &mut input)
                .expect("stdin description"),
            "first line\nsecond \\n literal"
        );

        fs::write(
            cwd.path().join("description.md"),
            "标题 / Заголовок\n\n中文段落\n",
        )
        .expect("description file");
        let file_cli = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            "description.md",
        ])
        .expect("file CLI");
        assert_eq!(
            resolve_profile_description(
                update_args(&file_cli),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("file description"),
            "标题 / Заголовок\n\n中文段落"
        );

        let empty_cli =
            Cli::try_parse_from(["cordy", "user", "profile", "update", "--description", ""])
                .expect("empty inline CLI");
        assert_eq!(
            resolve_profile_description(
                update_args(&empty_cli),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("empty inline clears"),
            ""
        );
    }

    #[test]
    fn profile_update_rejects_ambiguous_or_empty_input() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let ambiguous = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description",
            "inline",
            "--description-stdin",
        ])
        .expect("ambiguous CLI");
        assert!(resolve_profile_description(
            update_args(&ambiguous),
            &environment,
            &mut Cursor::new(b"stdin".to_vec())
        )
        .expect_err("ambiguous sources")
        .to_string()
        .contains("mutually exclusive"));

        let missing =
            Cli::try_parse_from(["cordy", "user", "profile", "update"]).expect("missing CLI");
        assert!(resolve_profile_description(
            update_args(&missing),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("missing source")
        .to_string()
        .contains("nothing to update"));

        let clear_with_input = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--clear",
            "--description",
            "inline",
        ])
        .expect("clear conflict CLI");
        assert!(resolve_profile_description(
            update_args(&clear_with_input),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("clear conflict")
        .to_string()
        .contains("--clear cannot be combined"));
    }

    #[test]
    fn profile_update_file_input_fails_closed_outside_workdir() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let outside = tempfile::tempdir().expect("outside dir");
        let external_path = outside.path().join("description.md");
        fs::write(&external_path, "external description").expect("external file");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let external_path = external_path.to_string_lossy().into_owned();
        let guarded = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            &external_path,
        ])
        .expect("guarded CLI");
        assert!(resolve_profile_description(
            update_args(&guarded),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("external file rejected")
        .to_string()
        .contains("--allow-external-file"));

        let allowed = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            &external_path,
            "--allow-external-file",
        ])
        .expect("allowed CLI");
        assert_eq!(
            resolve_profile_description(
                update_args(&allowed),
                &environment,
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect("external file allowed"),
            "external description"
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_update_rejects_workdir_symlink_that_escapes() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let outside = tempfile::tempdir().expect("outside dir");
        let external_path = outside.path().join("description.md");
        fs::write(&external_path, "escaped description").expect("external file");
        symlink(&external_path, cwd.path().join("description.md")).expect("symlink");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "user",
            "profile",
            "update",
            "--description-file",
            "description.md",
        ])
        .expect("symlink CLI");

        assert!(resolve_profile_description(
            update_args(&cli),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("escaping symlink rejected")
        .to_string()
        .contains("--allow-external-file"));
    }

    #[tokio::test]
    async fn workspace_list_authenticates_without_workspace_scope() {
        let app = Router::new().route(
            "/api/workspaces",
            get(|request: Request| async move {
                assert_eq!(request.headers()["authorization"], "Bearer workspace-token");
                assert!(request.headers().get("x-workspace-id").is_none());
                Json(serde_json::json!([
                    {"id":"11111111-1111-1111-1111-111111111111","name":"Alpha","slug":"alpha"},
                    {"id":"22222222-2222-2222-2222-222222222222","name":"Beta","slug":"beta"}
                ]))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "workspace-token");
        environment.set("CORDY_WORKSPACE_ID", "22222222-2222-2222-2222-222222222222");
        let cli = Cli::try_parse_from(["cordy", "workspace", "list", "--output", "json"])
            .expect("workspace list CLI");

        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("workspace list");

        let workspaces: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(workspaces.as_array().expect("workspace array").len(), 2);
        assert!(output.stderr.is_empty());
        server.abort();
    }

    #[test]
    fn workspace_table_marks_current_and_honors_full_id() {
        let workspaces = vec![
            WorkspaceSummary {
                id: "11111111-1111-1111-1111-111111111111".into(),
                name: "Alpha".into(),
                slug: "alpha".into(),
            },
            WorkspaceSummary {
                id: "22222222-2222-2222-2222-222222222222".into(),
                name: "Beta".into(),
                slug: "beta".into(),
            },
        ];
        assert_eq!(
            format_workspace_table(&workspaces, "22222222-2222-2222-2222-222222222222", false),
            "   ID        NAME   SLUG\n   11111111  Alpha  alpha\n*  22222222  Beta   beta\n"
        );
        let full = format_workspace_table(&workspaces, "", true);
        assert!(full.contains("11111111-1111-1111-1111-111111111111"));
        assert!(!full.contains("*  "));
    }

    #[tokio::test]
    async fn workspace_list_empty_and_missing_auth_match_go_messages() {
        let app = Router::new().route(
            "/api/workspaces",
            get(|| async { Json(serde_json::json!([])) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "workspace-token");
        let cli = Cli::try_parse_from(["cordy", "workspace", "list"]).expect("workspace list CLI");

        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("empty workspace list");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, "No workspaces found.\n");

        environment.set("CORDY_TOKEN", "");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing token");
        assert!(error
            .to_string()
            .contains("not authenticated: run 'cordy login' first"));
        server.abort();
    }

    #[tokio::test]
    async fn workspace_get_resolves_slug_but_bypasses_list_for_full_uuid() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let list_calls = Arc::new(AtomicUsize::new(0));
        let list_calls_by_handler = Arc::clone(&list_calls);
        let workspace_id = "22222222-2222-2222-2222-222222222222";
        let app = Router::new()
            .route(
                "/api/workspaces",
                get(move || {
                    let list_calls = Arc::clone(&list_calls_by_handler);
                    async move {
                        list_calls.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!([
                            {"id":"11111111-1111-1111-1111-111111111111","name":"Alpha","slug":"alpha"},
                            {"id":"22222222-2222-2222-2222-222222222222","name":"Beta","slug":"beta"}
                        ]))
                    }
                }),
            )
            .route(
                "/api/workspaces/22222222-2222-2222-2222-222222222222",
                get(|| async {
                    Json(serde_json::json!({
                        "id":"22222222-2222-2222-2222-222222222222",
                        "name":"Beta",
                        "slug":"beta",
                        "description":"Delivery workspace",
                        "context":"Product context"
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "workspace-token");

        for target in ["BETA", workspace_id] {
            let cli =
                Cli::try_parse_from(["cordy", "workspace", "get", target, "--output", "json"])
                    .expect("workspace get CLI");
            let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("workspace get");
            let workspace: Value = serde_json::from_str(&output.stdout).expect("JSON output");
            assert_eq!(workspace["id"], workspace_id);
        }
        assert_eq!(list_calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn workspace_reference_reports_ambiguous_and_missing_targets() {
        let workspaces = vec![
            WorkspaceSummary {
                id: "abcd1111-1111-1111-1111-111111111111".into(),
                name: "Alpha".into(),
                slug: "alpha".into(),
            },
            WorkspaceSummary {
                id: "abcd2222-2222-2222-2222-222222222222".into(),
                name: "Beta".into(),
                slug: "beta".into(),
            },
        ];
        let ambiguous = resolve_workspace_reference(&workspaces, "abcd")
            .expect_err("ambiguous prefix")
            .to_string();
        assert!(ambiguous.contains("ambiguous workspace id prefix \"abcd\""));
        assert!(ambiguous.contains("Alpha (alpha)"));
        assert!(ambiguous.contains("Beta (beta)"));
        assert!(resolve_workspace_reference(&workspaces, "gamma")
            .expect_err("missing slug")
            .to_string()
            .contains("run 'cordy workspace list'"));
        assert_eq!(
            resolve_workspace_reference(&workspaces, "ALPHA")
                .expect("case-insensitive slug")
                .id,
            workspaces[0].id
        );
    }

    #[test]
    fn workspace_details_table_truncates_description_and_context_at_sixty_chars() {
        let long = "界".repeat(61);
        let workspace = serde_json::json!({
            "id":"workspace-1",
            "name":"Alpha",
            "slug":"alpha",
            "description":long,
            "context":"x".repeat(60)
        });
        let table = format_workspace_details_table(&workspace);
        assert!(table.contains(&("界".repeat(57) + "...")));
        assert!(table.contains(&"x".repeat(60)));
        assert!(!table.contains(&"界".repeat(58)));
    }

    #[tokio::test]
    async fn workspace_get_without_argument_requires_default_workspace() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from(["cordy", "workspace", "get"]).expect("workspace get CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("missing default workspace");
        assert!(error.to_string().contains(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        ));
    }

    #[tokio::test]
    async fn workspace_create_posts_complete_body_without_workspace_scope() {
        let captured = Arc::new(Mutex::new(None));
        let captured_by_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/workspaces",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_by_handler);
                async move {
                    assert_eq!(headers["authorization"], "Bearer workspace-token");
                    assert!(headers.get("x-workspace-id").is_none());
                    *captured.lock().expect("capture body") = Some(body.clone());
                    Json(serde_json::json!({
                        "id":"33333333-3333-3333-3333-333333333333",
                        "name":body["name"],
                        "slug":body["slug"],
                        "description":body["description"],
                        "context":body["context"]
                    }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "workspace-token");
        environment.set("CORDY_WORKSPACE_ID", "must-not-be-sent");
        let cli = Cli::try_parse_from([
            "cordy",
            "workspace",
            "create",
            "--name",
            "Support Team",
            "--slug",
            "support-team",
            "--description",
            r"First line\nSecond line",
            "--context-stdin",
            "--issue-prefix",
            "SUP",
            "--output",
            "table",
        ])
        .expect("workspace create CLI");
        let output = run_with_input(
            &cli,
            &environment,
            &mut Cursor::new(b"Customer support context\n".to_vec()),
        )
        .await
        .expect("create workspace");

        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("request body");
        assert_eq!(body["name"], "Support Team");
        assert_eq!(body["slug"], "support-team");
        assert_eq!(body["description"], "First line\nSecond line");
        assert_eq!(body["context"], "Customer support context");
        assert_eq!(body["issue_prefix"], "SUP");
        assert!(output.stdout.starts_with("ID"));
        assert!(output.stdout.contains("support-team"));
        server.abort();
    }

    #[test]
    fn workspace_create_validates_required_and_safe_input_flags() {
        let missing_name =
            Cli::try_parse_from(["cordy", "workspace", "create", "--slug", "support-team"])
                .expect("missing name CLI");
        assert_eq!(
            build_workspace_create_body(
                create_workspace_args(&missing_name),
                &mut Cursor::new(Vec::<u8>::new())
            )
            .expect_err("missing name")
            .to_string(),
            "--name is required"
        );

        let dual_stdin = Cli::try_parse_from([
            "cordy",
            "workspace",
            "create",
            "--name",
            "Support",
            "--slug",
            "support",
            "--description-stdin",
            "--context-stdin",
        ])
        .expect("dual stdin CLI");
        assert!(build_workspace_create_body(
            create_workspace_args(&dual_stdin),
            &mut Cursor::new(b"ambiguous".to_vec())
        )
        .expect_err("dual stdin")
        .to_string()
        .contains("a single stdin cannot feed both fields"));

        let empty_prefix = Cli::try_parse_from([
            "cordy",
            "workspace",
            "create",
            "--name",
            "Support",
            "--slug",
            "support",
            "--issue-prefix",
            "   ",
        ])
        .expect("empty prefix CLI");
        assert!(build_workspace_create_body(
            create_workspace_args(&empty_prefix),
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("empty issue prefix")
        .to_string()
        .contains("omit it to use the server-generated prefix"));
    }

    #[tokio::test]
    async fn workspace_update_resolves_slug_and_patches_without_switching_default() {
        let captured = Arc::new(Mutex::new(None));
        let captured_by_handler = Arc::clone(&captured);
        let workspace_id = "44444444-4444-4444-4444-444444444444";
        let app = Router::new()
            .route(
                "/api/workspaces",
                get(|| async {
                    Json(serde_json::json!([{
                        "id":"44444444-4444-4444-4444-444444444444",
                        "name":"Before",
                        "slug":"delivery"
                    }]))
                }),
            )
            .route(
                "/api/workspaces/44444444-4444-4444-4444-444444444444",
                patch(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_by_handler);
                    async move {
                        assert_eq!(headers["x-workspace-id"], "original-default");
                        *captured.lock().expect("capture body") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"44444444-4444-4444-4444-444444444444",
                            "name":body["name"],
                            "slug":"delivery",
                            "description":body["description"],
                            "context":"Existing context"
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let config_dir = home.path().join(".cordy");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("config.json"),
            format!(
                r#"{{"server_url":"http://{address}","token":"workspace-token","workspace_id":"original-default"}}"#
            ),
        )
        .expect("config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "workspace",
            "update",
            "delivery",
            "--name",
            "After",
            "--description",
            "",
            "--output",
            "json",
        ])
        .expect("workspace update CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update workspace");

        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("request body");
        assert_eq!(body["name"], "After");
        assert_eq!(body["description"], "");
        assert_eq!(
            serde_json::from_str::<Value>(&output.stdout).expect("JSON")["id"],
            workspace_id
        );
        assert_eq!(
            environment
                .load_config("")
                .expect("config after update")
                .workspace_id,
            "original-default"
        );
        server.abort();
    }

    #[tokio::test]
    async fn workspace_update_rejects_no_changes_before_api_setup() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let cli = Cli::try_parse_from([
            "cordy",
            "workspace",
            "update",
            "55555555-5555-5555-5555-555555555555",
        ])
        .expect("empty workspace update CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("no changes");
        assert_eq!(
            error.to_string(),
            "no fields to update; use --name, --description, --context, or --issue-prefix"
        );
    }

    #[test]
    fn workspace_update_supports_safe_files_and_rejects_ambiguous_or_empty_changes() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("context.md"), "First\nSecond \\n literal\n")
            .expect("context file");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let file_cli = Cli::try_parse_from([
            "cordy",
            "workspace",
            "update",
            "workspace-id",
            "--context-file",
            "context.md",
        ])
        .expect("file CLI");
        let body = build_workspace_update_body(
            update_workspace_args(&file_cli),
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .expect("file body");
        assert_eq!(body["context"], "First\nSecond \\n literal");

        let ambiguous = Cli::try_parse_from([
            "cordy",
            "workspace",
            "update",
            "workspace-id",
            "--description",
            "inline",
            "--description-file",
            "context.md",
        ])
        .expect("ambiguous CLI");
        assert!(build_workspace_update_body(
            update_workspace_args(&ambiguous),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("ambiguous description")
        .to_string()
        .contains("mutually exclusive"));

        let empty = Cli::try_parse_from(["cordy", "workspace", "update", "workspace-id"])
            .expect("empty CLI");
        assert!(build_workspace_update_body(
            update_workspace_args(&empty),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect("empty body")
        .is_empty());

        let empty_prefix = Cli::try_parse_from([
            "cordy",
            "workspace",
            "update",
            "workspace-id",
            "--issue-prefix",
            " ",
        ])
        .expect("empty prefix CLI");
        assert!(build_workspace_update_body(
            update_workspace_args(&empty_prefix),
            &environment,
            &mut Cursor::new(Vec::<u8>::new())
        )
        .expect_err("empty issue prefix")
        .to_string()
        .contains("clearing the prefix is not supported"));
    }

    #[test]
    fn table_output_matches_go_vertical_table_contract() {
        let profile = serde_json::json!({"id":"user-1","name":"Ada","email":"ada@example.com"});
        assert_eq!(
            format_user_profile_table(&profile),
            "ID                   user-1\nNAME                 Ada\nEMAIL                ada@example.com\nPROFILE DESCRIPTION  (not set)\n"
        );
    }

    #[test]
    fn daemon_context_never_falls_back_to_owner_credentials() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let config_dir = home.path().join(".cordy");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("config.json"),
            r#"{"server_url":"https://api.example.com","token":"mul_owner"}"#,
        )
        .expect("config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_AGENT_ID", "agent-1");
        let cli = Cli::try_parse_from(["cordy", "user", "profile", "get"]).expect("parse CLI");

        let error = new_api_client(&cli, &environment).expect_err("must fail closed");
        assert!(error.to_string().contains("task-scoped mat_ token"));
    }

    #[test]
    fn websocket_server_urls_normalize_to_http_api_base() {
        assert_eq!(
            normalize_api_base_url("wss://api.cordy.ai/ws?old=1#fragment").expect("URL"),
            "https://api.cordy.ai"
        );
    }
}
