use clap::{Args, Subcommand};

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct TeamArgs {
    #[command(subcommand)]
    pub(super) command: TeamCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum TeamCommand {
    #[command(about = "List teams in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Get team details")]
    Get {
        #[arg(value_name = "TEAM-ID")]
        team_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Create a new team")]
    Create(TeamCreateArgs),
    #[command(about = "Update a team")]
    Update(TeamUpdateArgs),
    #[command(about = "Delete (archive) a team")]
    Delete {
        #[arg(value_name = "TEAM-ID")]
        team_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Work with team members")]
    Member(TeamMemberArgs),
    #[command(about = "Record a team leader evaluation on an issue")]
    Activity(TeamActivityArgs),
}

#[derive(Debug, Args)]
pub(super) struct TeamCreateArgs {
    #[arg(long, help = "Team name (required)")]
    pub(super) name: Option<String>,
    #[arg(long, default_value = "", help = "Team description")]
    pub(super) description: String,
    #[arg(long, help = "Leader agent (name or ID) — required")]
    pub(super) leader: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct TeamUpdateArgs {
    #[arg(value_name = "TEAM-ID")]
    pub(super) team_id: String,
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
pub(super) struct TeamMemberArgs {
    #[command(subcommand)]
    pub(super) command: TeamMemberCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum TeamMemberCommand {
    #[command(about = "List members of a team")]
    List {
        #[arg(value_name = "TEAM-ID")]
        team_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add a member to a team")]
    Add(TeamMemberAddArgs),
    #[command(about = "Change a team member's role")]
    SetRole(TeamMemberSetRoleArgs),
    #[command(about = "Remove a member from a team")]
    Remove(TeamMemberRemoveArgs),
}

#[derive(Debug, Args)]
pub(super) struct TeamMemberAddArgs {
    #[arg(value_name = "TEAM-ID")]
    pub(super) team_id: String,
    #[arg(long, help = "Member or agent ID (required)")]
    pub(super) member_id: Option<String>,
    #[arg(
        long = "type",
        default_value = "agent",
        help = "Member type: agent or member"
    )]
    pub(super) member_type: String,
    #[arg(long, default_value = "member", help = "Role in the team")]
    pub(super) role: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct TeamMemberSetRoleArgs {
    #[arg(value_name = "TEAM-ID")]
    pub(super) team_id: String,
    #[arg(long, help = "Member or agent ID (required)")]
    pub(super) member_id: Option<String>,
    #[arg(
        long = "member-type",
        default_value = "agent",
        help = "Member type: agent or member"
    )]
    pub(super) member_type: String,
    #[arg(long, help = "New role in the team (required)")]
    pub(super) role: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct TeamMemberRemoveArgs {
    #[arg(value_name = "TEAM-ID")]
    pub(super) team_id: String,
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
pub(super) struct TeamActivityArgs {
    #[arg(value_name = "ISSUE-ID")]
    pub(super) issue_id: String,
    #[arg(value_name = "OUTCOME")]
    pub(super) outcome: String,
    #[arg(long, default_value = "", help = "Short explanation of the decision")]
    pub(super) reason: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
