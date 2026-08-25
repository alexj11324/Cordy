use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct PropertyArgs {
    #[command(subcommand)]
    pub(super) command: PropertyCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum PropertyCommand {
    #[command(about = "List property definitions")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Include archived properties")]
        include_archived: bool,
    },
    #[command(about = "Show one property definition")]
    Get {
        #[arg(value_name = "ID-OR-NAME")]
        property: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a property definition (workspace owner/admin only)")]
    Create(PropertyCreateArgs),
    #[command(about = "Update a property definition (owner/admin only; type is immutable)")]
    Update(PropertyUpdateArgs),
    #[command(about = "Archive a property definition (hidden from pickers; values preserved)")]
    Archive(PropertyArchiveArgs),
    #[command(about = "Restore an archived property definition")]
    Unarchive(PropertyArchiveArgs),
}

#[derive(Debug, Args)]
pub(super) struct PropertyCreateArgs {
    #[arg(long, help = "Property name (required)")]
    pub(super) name: Option<String>,
    #[arg(
        long = "type",
        help = "Property type: text, number, select, multi_select, date, checkbox, url, actor, multi_actor (required)"
    )]
    pub(super) property_type: Option<String>,
    #[arg(long, default_value = "", help = "Property description")]
    pub(super) description: String,
    #[arg(
        long,
        default_value = "",
        help = "Property icon key from the Web picker (for example, flag, tag, or shield)"
    )]
    pub(super) icon: String,
    #[arg(long, action = clap::ArgAction::Append, help = "Select option as \"Name\" or \"Name:#rrggbb\" (repeatable; select types only)")]
    pub(super) option: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct PropertyUpdateArgs {
    #[arg(value_name = "ID-OR-NAME")]
    pub(super) property: String,
    #[arg(long, help = "New property name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "New property description")]
    pub(super) description: Option<String>,
    #[arg(
        long,
        help = "New property icon key from the Web picker; pass an empty value to clear"
    )]
    pub(super) icon: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Replacement option list as \"Name\" or \"Name:#rrggbb\" (repeatable)")]
    pub(super) option: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct PropertyArchiveArgs {
    #[arg(value_name = "ID-OR-NAME")]
    pub(super) property: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
