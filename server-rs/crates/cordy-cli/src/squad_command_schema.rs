use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct SquadArgs {
    #[command(subcommand)]
    pub(super) command: SquadCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum SquadCommand {
    #[command(about = "List squads in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Get squad details")]
    Get {
        #[arg(value_name = "SQUAD-ID")]
        squad_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Create a new squad")]
    Create(SquadCreateArgs),
    #[command(about = "Update a squad")]
    Update(SquadUpdateArgs),
    #[command(about = "Delete (archive) a squad")]
    Delete {
        #[arg(value_name = "SQUAD-ID")]
        squad_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Work with squad members")]
    Member(SquadMemberArgs),
    #[command(about = "Record a squad leader evaluation on an issue")]
    Activity(SquadActivityArgs),
}

#[derive(Debug, Args)]
pub(super) struct SquadCreateArgs {
    #[arg(long, help = "Squad name (required)")]
    pub(super) name: Option<String>,
    #[arg(long, default_value = "", help = "Squad description")]
    pub(super) description: String,
    #[arg(long, help = "Leader agent (name or ID) — required")]
    pub(super) leader: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SquadUpdateArgs {
    #[arg(value_name = "SQUAD-ID")]
    pub(super) squad_id: String,
    #[arg(long, help = "New name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "New description")]
    pub(super) description: Option<String>,
    #[arg(long, help = "New instructions")]
    pub(super) instructions: Option<String>,
    #[arg(long, help = "New leader agent (name or ID)")]
    pub(super) leader: Option<String>,
    #[arg(long, help = "New avatar URL")]
    pub(super) avatar_url: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SquadMemberArgs {
    #[command(subcommand)]
    pub(super) command: SquadMemberCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum SquadMemberCommand {
    #[command(about = "List members of a squad")]
    List {
        #[arg(value_name = "SQUAD-ID")]
        squad_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add a member to a squad")]
    Add(SquadMemberAddArgs),
    #[command(about = "Change a squad member's role")]
    SetRole(SquadMemberSetRoleArgs),
    #[command(about = "Remove a member from a squad")]
    Remove(SquadMemberRemoveArgs),
}

#[derive(Debug, Args)]
pub(super) struct SquadMemberAddArgs {
    #[arg(value_name = "SQUAD-ID")]
    pub(super) squad_id: String,
    #[arg(long, help = "Member or agent ID (required)")]
    pub(super) member_id: Option<String>,
    #[arg(
        long = "type",
        default_value = "agent",
        help = "Member type: agent or member"
    )]
    pub(super) member_type: String,
    #[arg(long, default_value = "member", help = "Role in the squad")]
    pub(super) role: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SquadMemberSetRoleArgs {
    #[arg(value_name = "SQUAD-ID")]
    pub(super) squad_id: String,
    #[arg(long, help = "Member or agent ID (required)")]
    pub(super) member_id: Option<String>,
    #[arg(
        long = "member-type",
        default_value = "agent",
        help = "Member type: agent or member"
    )]
    pub(super) member_type: String,
    #[arg(long, help = "New role in the squad (required)")]
    pub(super) role: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SquadMemberRemoveArgs {
    #[arg(value_name = "SQUAD-ID")]
    pub(super) squad_id: String,
    #[arg(long, help = "Member or agent ID (required)")]
    pub(super) member_id: Option<String>,
    #[arg(
        long = "type",
        default_value = "agent",
        help = "Member type: agent or member"
    )]
    pub(super) member_type: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct SquadActivityArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(value_name = "OUTCOME")]
    pub(super) outcome: String,
    #[arg(long, default_value = "", help = "Short explanation of the decision")]
    pub(super) reason: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
