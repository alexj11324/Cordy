use clap::{Args, Subcommand};
use std::path::PathBuf;

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct WorkspaceArgs {
    #[command(subcommand)]
    pub(super) command: WorkspaceCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum WorkspaceCommand {
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
    #[command(about = "Set the default workspace for this profile")]
    Switch {
        #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
        workspace: String,
    },
    #[command(about = "Manage workspace members")]
    Member(WorkspaceMemberArgs),
    #[command(about = "Manage the workspace's MCP server library")]
    Mcp(WorkspaceMcpArgs),
}

#[derive(Debug, Args)]
pub(super) struct WorkspaceMcpArgs {
    #[command(subcommand)]
    pub(super) command: WorkspaceMcpCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum WorkspaceMcpCommand {
    #[command(about = "List the workspace's MCP servers")]
    List {
        #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
        workspace: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Add an MCP server to the workspace library (admin/owner only)")]
    Add(WorkspaceMcpAddArgs),
    #[command(about = "Rename or replace a workspace MCP server (admin/owner only)")]
    Update(WorkspaceMcpUpdateArgs),
    #[command(about = "Remove an MCP server from the workspace library (admin/owner only)")]
    Remove {
        #[arg(value_name = "SERVER-ID")]
        server_id: String,
        #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
        workspace: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
}

#[derive(Debug, Args)]
pub(super) struct WorkspaceMcpAddArgs {
    #[arg(value_name = "SERVER-NAME")]
    pub(super) server_name: String,
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    pub(super) workspace: Option<String>,
    #[arg(long, help = "Server entry as JSON (avoid: lands in shell history)")]
    pub(super) server_config: Option<String>,
    #[arg(long, help = "Read the server entry JSON from stdin")]
    pub(super) server_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the server entry JSON from a file"
    )]
    pub(super) server_config_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct WorkspaceMcpUpdateArgs {
    #[arg(value_name = "SERVER-ID")]
    pub(super) server_id: String,
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    pub(super) workspace: Option<String>,
    #[arg(long, help = "New server name")]
    pub(super) name: Option<String>,
    #[arg(
        long,
        help = "Replacement server entry as JSON (avoid: lands in shell history)"
    )]
    pub(super) server_config: Option<String>,
    #[arg(long, help = "Read the replacement server entry JSON from stdin")]
    pub(super) server_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the replacement server entry JSON from a file"
    )]
    pub(super) server_config_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct WorkspaceMemberArgs {
    #[command(subcommand)]
    pub(super) command: WorkspaceMemberCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum WorkspaceMemberCommand {
    #[command(about = "List workspace members")]
    List {
        #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
        workspace: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Invite a member to a workspace by email")]
    Invite(WorkspaceMemberInviteArgs),
}

#[derive(Debug, Args)]
pub(super) struct WorkspaceMemberInviteArgs {
    #[arg(value_name = "EMAIL")]
    pub(super) email: String,
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    pub(super) workspace: Option<String>,
    #[arg(
        long,
        default_value = "member",
        help = "Member role to grant: member or admin (owner is not allowed)"
    )]
    pub(super) role: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct CreateWorkspaceArgs {
    #[arg(long, help = "Workspace name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "Workspace slug")]
    pub(super) slug: Option<String>,
    #[arg(
        long,
        help = "Workspace description (decodes \\n, \\r, \\t, \\\\; use --description-stdin to preserve literal backslashes)"
    )]
    pub(super) description: Option<String>,
    #[arg(
        long,
        help = "Read description from stdin (preserves multi-line content verbatim)"
    )]
    pub(super) description_stdin: bool,
    #[arg(
        long,
        help = "Workspace context (decodes \\n, \\r, \\t, \\\\; use --context-stdin to preserve literal backslashes)"
    )]
    pub(super) context: Option<String>,
    #[arg(
        long,
        help = "Read context from stdin (preserves multi-line content verbatim)"
    )]
    pub(super) context_stdin: bool,
    #[arg(long, help = "Issue prefix (uppercased server-side)")]
    pub(super) issue_prefix: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct UpdateWorkspaceArgs {
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    pub(super) workspace: Option<String>,
    #[arg(long, help = "New workspace name")]
    pub(super) name: Option<String>,
    #[arg(
        long,
        help = "New description; pass an empty value to clear (decodes \\n, \\r, \\t, \\\\; use stdin/file to preserve literal backslashes)"
    )]
    pub(super) description: Option<String>,
    #[arg(
        long,
        help = "Read description from stdin (preserves multi-line content verbatim)"
    )]
    pub(super) description_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read description from a UTF-8 file")]
    pub(super) description_file: Option<PathBuf>,
    #[arg(
        long,
        help = "New context; pass an empty value to clear (decodes \\n, \\r, \\t, \\\\; use stdin/file to preserve literal backslashes)"
    )]
    pub(super) context: Option<String>,
    #[arg(
        long,
        help = "Read context from stdin (preserves multi-line content verbatim)"
    )]
    pub(super) context_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read context from a UTF-8 file")]
    pub(super) context_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Allow description/context files outside the current working directory"
    )]
    pub(super) allow_external_file: bool,
    #[arg(long, help = "New issue prefix (uppercased server-side)")]
    pub(super) issue_prefix: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
