use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct RuntimeArgs {
    #[command(subcommand)]
    pub(super) command: RuntimeCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum RuntimeCommand {
    #[command(about = "List runtimes in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Get token usage for a runtime")]
    Usage {
        #[arg(value_name = "RUNTIME-ID")]
        runtime_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(
            long,
            default_value_t = 90,
            help = "Number of days of usage data (max 365)"
        )]
        days: i32,
    },
    #[command(about = "Get hourly task activity for a runtime")]
    Activity {
        #[arg(value_name = "RUNTIME-ID")]
        runtime_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Set a custom display name for a runtime")]
    Rename {
        #[arg(value_name = "RUNTIME-ID")]
        runtime_id: String,
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(long, help = "Apply the name to every runtime on the same machine")]
        machine: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Delete a runtime from the workspace")]
    Delete {
        #[arg(value_name = "RUNTIME-ID")]
        runtime_id: String,
        #[arg(long, help = "Unbind active agents, cancel their tasks, then delete")]
        cascade: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Initiate a CLI update on a runtime")]
    Update {
        #[arg(value_name = "RUNTIME-ID")]
        runtime_id: String,
        #[arg(long, help = "Target version to update to (required)")]
        target_version: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
        #[arg(long, help = "Wait for update to complete")]
        wait: bool,
    },
    #[command(about = "Manage custom runtime profiles")]
    Profile(RuntimeProfileArgs),
}

#[derive(Debug, Args)]
pub(super) struct RuntimeProfileArgs {
    #[command(subcommand)]
    pub(super) command: RuntimeProfileCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum RuntimeProfileCommand {
    #[command(about = "List custom runtime profiles in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Create a custom runtime profile")]
    Create(RuntimeProfileCreateArgs),
    #[command(about = "Update a custom runtime profile")]
    Update(RuntimeProfileUpdateArgs),
    #[command(about = "Delete a custom runtime profile")]
    Delete {
        #[arg(value_name = "PROFILE-ID")]
        profile_id: String,
    },
    #[command(about = "Pin a per-machine executable path for a runtime profile")]
    SetPath {
        #[arg(value_name = "PROFILE-ID")]
        profile_id: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Absolute executable path (required)"
        )]
        path: Option<String>,
    },
    #[command(about = "Remove a per-machine executable path override")]
    UnsetPath {
        #[arg(value_name = "PROFILE-ID")]
        profile_id: String,
    },
}

#[derive(Debug, Args)]
pub(super) struct RuntimeProfileCreateArgs {
    #[arg(long, help = "Supported backend the profile routes to (required)")]
    pub(super) protocol_family: Option<String>,
    #[arg(long, help = "Executable the daemon resolves on PATH (required)")]
    pub(super) command_name: Option<String>,
    #[arg(long, help = "Human-readable profile name (required)")]
    pub(super) display_name: Option<String>,
    #[arg(long, default_value = "", help = "Optional description")]
    pub(super) description: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct RuntimeProfileUpdateArgs {
    #[arg(value_name = "PROFILE-ID")]
    pub(super) profile_id: String,
    #[arg(long, help = "New display name")]
    pub(super) display_name: Option<String>,
    #[arg(long, help = "New command name")]
    pub(super) command_name: Option<String>,
    #[arg(long, help = "New description")]
    pub(super) description: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Enable or disable the profile")]
    pub(super) enabled: Option<bool>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
