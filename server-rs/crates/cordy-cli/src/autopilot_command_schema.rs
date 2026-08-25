use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct AutopilotArgs {
    #[command(subcommand)]
    pub(super) command: AutopilotCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AutopilotCommand {
    #[command(about = "List autopilots in the workspace")]
    List {
        #[arg(long, default_value = "", help = "Filter by status (active, paused)")]
        status: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Show full UUIDs in table output")]
        full_id: bool,
    },
    #[command(about = "Get autopilot details (includes triggers)")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a new autopilot")]
    Create(AutopilotCreateArgs),
    #[command(about = "Update an autopilot")]
    Update(AutopilotUpdateArgs),
    #[command(about = "Delete an autopilot")]
    Delete {
        #[arg(value_name = "ID")]
        id: String,
    },
    #[command(about = "Manually trigger an autopilot to run once")]
    Trigger {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "List execution history for an autopilot")]
    Runs {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, default_value_t = 20, help = "Max number of runs to return")]
        limit: i32,
        #[arg(long, default_value_t = 0, help = "Pagination offset")]
        offset: i32,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add a schedule or webhook trigger to an autopilot")]
    TriggerAdd(AutopilotTriggerAddArgs),
    #[command(about = "Update an existing trigger")]
    TriggerUpdate(AutopilotTriggerUpdateArgs),
    #[command(about = "Delete a trigger")]
    TriggerDelete {
        #[arg(value_name = "AUTOPILOT-ID")]
        autopilot_id: String,
        #[arg(value_name = "TRIGGER-ID")]
        trigger_id: String,
    },
    #[command(about = "Rotate the webhook URL of a webhook trigger")]
    TriggerRotateUrl(AutopilotTriggerRotateUrlArgs),
}

#[derive(Debug, Args)]
pub(super) struct AutopilotTriggerAddArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    pub(super) autopilot_id: String,
    #[arg(
        long,
        default_value = "schedule",
        help = "Trigger kind: schedule or webhook"
    )]
    pub(super) kind: String,
    #[arg(
        long,
        default_value = "",
        help = "Cron expression (required for --kind schedule)"
    )]
    pub(super) cron: String,
    #[arg(
        long,
        default_value = "",
        help = "IANA timezone (default UTC; schedule only)"
    )]
    pub(super) timezone: String,
    #[arg(long, default_value = "", help = "Optional human-readable label")]
    pub(super) label: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotTriggerUpdateArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    pub(super) autopilot_id: String,
    #[arg(value_name = "TRIGGER-ID")]
    pub(super) trigger_id: String,
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub(super) enabled: Option<bool>,
    #[arg(long)]
    pub(super) cron: Option<String>,
    #[arg(long)]
    pub(super) timezone: Option<String>,
    #[arg(long)]
    pub(super) label: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotTriggerRotateUrlArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    pub(super) autopilot_id: String,
    #[arg(value_name = "TRIGGER-ID")]
    pub(super) trigger_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
    #[arg(short = 'y', long, help = "Skip the interactive confirmation prompt")]
    pub(super) yes: bool,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotCreateArgs {
    #[arg(long, help = "Autopilot title (required)")]
    pub(super) title: Option<String>,
    #[arg(
        long,
        default_value = "",
        help = "Autopilot description (used as task prompt)"
    )]
    pub(super) description: String,
    #[arg(long, help = "Assignee agent (name or ID) — required")]
    pub(super) agent: Option<String>,
    #[arg(long, help = "Execution mode: create_issue or run_only (required)")]
    pub(super) mode: Option<String>,
    #[arg(
        long,
        help = "Priority for created issues (none, low, medium, high, urgent)"
    )]
    pub(super) priority: Option<String>,
    #[arg(long, default_value = "", help = "Project ID (optional)")]
    pub(super) project: String,
    #[arg(
        long,
        default_value = "",
        help = "Template for issue titles (create_issue mode). Only {{date}} (UTC, YYYY-MM-DD) is interpolated; any other {{...}} token is rejected at create-time."
    )]
    pub(super) issue_title_template: String,
    #[arg(
        long,
        action = clap::ArgAction::Append,
        help = "Member subscriber to notify for issues this autopilot creates (name or user ID; repeatable)"
    )]
    pub(super) subscriber: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AutopilotUpdateArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long)]
    pub(super) title: Option<String>,
    #[arg(long)]
    pub(super) description: Option<String>,
    #[arg(long, help = "New assignee agent (name or ID)")]
    pub(super) agent: Option<String>,
    #[arg(long, help = "New project ID (use empty string to clear)")]
    pub(super) project: Option<String>,
    #[arg(long)]
    pub(super) priority: Option<String>,
    #[arg(long, help = "New status (active, paused)")]
    pub(super) status: Option<String>,
    #[arg(long, help = "New execution mode (create_issue or run_only)")]
    pub(super) mode: Option<String>,
    #[arg(
        long,
        help = "New issue title template. Only {{date}} is interpolated."
    )]
    pub(super) issue_title_template: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Replace subscribers with this member (repeatable)")]
    pub(super) subscriber: Vec<String>,
    #[arg(long, help = "Remove all autopilot subscribers")]
    pub(super) clear_subscribers: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
