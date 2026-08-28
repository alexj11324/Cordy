use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct LabelArgs {
    #[command(subcommand)]
    pub(super) command: LabelCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum LabelCommand {
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
pub(super) struct LabelCreateArgs {
    #[arg(long, help = "Label name (required)")]
    pub(super) name: Option<String>,
    #[arg(long, help = "Hex color like #3b82f6 (required)")]
    pub(super) color: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct LabelUpdateArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "New name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "New hex color")]
    pub(super) color: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
