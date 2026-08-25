use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct ProjectArgs {
    #[command(subcommand)]
    pub(super) command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ProjectCommand {
    #[command(about = "List projects in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
        #[arg(long, help = "Filter by status")]
        status: Option<String>,
    },
    #[command(about = "Get project details")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a new project")]
    Create(ProjectCreateArgs),
    #[command(about = "Update a project")]
    Update(ProjectUpdateArgs),
    #[command(about = "Delete a project")]
    Delete {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Change project status")]
    Status {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(value_name = "STATUS")]
        status: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Manage resources attached to a project")]
    Resource(ProjectResourceArgs),
}

#[derive(Debug, Args)]
pub(super) struct ProjectResourceArgs {
    #[command(subcommand)]
    pub(super) command: ProjectResourceCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ProjectResourceCommand {
    #[command(about = "List resources attached to a project")]
    List {
        #[arg(value_name = "PROJECT-ID")]
        project_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
    },
    #[command(about = "Attach a resource to a project (e.g. --type github_repo --url <url>)")]
    Add(ProjectResourceAddArgs),
    #[command(about = "Edit an attached resource (ref payload, label, or position)")]
    Update(ProjectResourceUpdateArgs),
    #[command(about = "Detach a resource from a project")]
    Remove {
        #[arg(value_name = "PROJECT-ID")]
        project_id: String,
        #[arg(value_name = "RESOURCE-ID")]
        resource_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
}

#[derive(Debug, Args)]
pub(super) struct ProjectResourceAddArgs {
    #[arg(value_name = "PROJECT-ID")]
    pub(super) project_id: String,
    #[arg(
        long = "type",
        default_value = "github_repo",
        help = "Resource type (e.g. github_repo, local_directory — see docs)"
    )]
    pub(super) resource_type: String,
    #[arg(
        long,
        help = "Shortcut: the repo URL (only used when --type github_repo)"
    )]
    pub(super) url: Option<String>,
    #[arg(
        long,
        help = "Shortcut: optional default branch hint (only used when --type github_repo)"
    )]
    pub(super) default_branch_hint: Option<String>,
    #[arg(
        long,
        help = "Shortcut: absolute path to the working directory (only used when --type local_directory)"
    )]
    pub(super) local_path: Option<String>,
    #[arg(
        long,
        help = "Shortcut: id of the daemon that owns the local path (only used when --type local_directory)"
    )]
    pub(super) daemon_id: Option<String>,
    #[arg(
        long,
        help = "Shortcut: optional label embedded in resource_ref (only used when --type local_directory)"
    )]
    pub(super) ref_label: Option<String>,
    #[arg(
        long,
        help = "Shortcut: how tasks share the directory — in_place (default, one task at a time) or worktree (each task gets its own git worktree; requires a git repo) (only used when --type local_directory)"
    )]
    pub(super) execution_mode: Option<String>,
    #[arg(
        long = "ref",
        help = "Generic JSON resource_ref payload, or a github_repo checkout ref when used with --url"
    )]
    pub(super) resource_ref: Option<String>,
    #[arg(long, help = "Optional human-readable label")]
    pub(super) label: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct ProjectResourceUpdateArgs {
    #[arg(value_name = "PROJECT-ID")]
    pub(super) project_id: String,
    #[arg(value_name = "RESOURCE-ID")]
    pub(super) resource_id: String,
    #[arg(long, help = "Shortcut: new repo URL (github_repo)")]
    pub(super) url: Option<String>,
    #[arg(long, help = "Shortcut: new default branch hint (github_repo)")]
    pub(super) default_branch_hint: Option<String>,
    #[arg(long, help = "Shortcut: new absolute local path (local_directory)")]
    pub(super) local_path: Option<String>,
    #[arg(long, help = "Shortcut: new daemon id (local_directory)")]
    pub(super) daemon_id: Option<String>,
    #[arg(
        long,
        help = "Shortcut: new label embedded in resource_ref (local_directory)"
    )]
    pub(super) ref_label: Option<String>,
    #[arg(
        long,
        help = "Shortcut: new execution mode — in_place or worktree (local_directory)"
    )]
    pub(super) execution_mode: Option<String>,
    #[arg(
        long = "ref",
        help = "Generic JSON resource_ref payload, or a github_repo checkout ref"
    )]
    pub(super) resource_ref: Option<String>,
    #[arg(long, help = "New human-readable label; pass an empty string to clear")]
    pub(super) label: Option<String>,
    #[arg(long, help = "Clear the human-readable label")]
    pub(super) clear_label: bool,
    #[arg(long, help = "New display position")]
    pub(super) position: Option<i32>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct ProjectCreateArgs {
    #[arg(long, help = "Project title (required)")]
    pub(super) title: Option<String>,
    #[arg(long, help = "Project description")]
    pub(super) description: Option<String>,
    #[arg(long, help = "Project status")]
    pub(super) status: Option<String>,
    #[arg(long, help = "Project icon (emoji)")]
    pub(super) icon: Option<String>,
    #[arg(long, help = "Lead name (member or agent)")]
    pub(super) lead: Option<String>,
    #[arg(long, help = "Start date (calendar day, YYYY-MM-DD)")]
    pub(super) start_date: Option<String>,
    #[arg(long, help = "Due date (calendar day, YYYY-MM-DD)")]
    pub(super) due_date: Option<String>,
    #[arg(
        long,
        action = clap::ArgAction::Append,
        help = "Attach a github_repo resource by URL"
    )]
    pub(super) repo: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct ProjectUpdateArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "New title")]
    pub(super) title: Option<String>,
    #[arg(long, help = "New description")]
    pub(super) description: Option<String>,
    #[arg(long, help = "New status")]
    pub(super) status: Option<String>,
    #[arg(long, help = "New icon (emoji)")]
    pub(super) icon: Option<String>,
    #[arg(long, help = "New lead name (member or agent)")]
    pub(super) lead: Option<String>,
    #[arg(long, help = "New start date; pass an empty string to clear")]
    pub(super) start_date: Option<String>,
    #[arg(long, help = "New due date; pass an empty string to clear")]
    pub(super) due_date: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
