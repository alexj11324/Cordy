use clap::{Args, Subcommand};
use std::path::PathBuf;

use super::OutputFormat;

#[derive(Debug, Args)]
pub(super) struct AgentArgs {
    #[command(subcommand)]
    pub(super) command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AgentCommand {
    #[command(about = "List agents in the workspace")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Include archived agents")]
        include_archived: bool,
    },
    #[command(about = "Get agent details")]
    Get {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a new agent")]
    Create(AgentCreateArgs),
    #[command(about = "Update an agent")]
    Update(AgentUpdateArgs),
    #[command(about = "Archive an agent")]
    Archive {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Restore an archived agent")]
    Restore {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "List tasks for an agent")]
    Tasks {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Upload an avatar image for an agent")]
    Avatar {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(
            long,
            value_name = "PATH",
            help = "Path to the avatar image file (required)"
        )]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Manage agent skill assignments")]
    Skills(AgentSkillsArgs),
    #[command(about = "Read and update an agent's custom environment variables (audited)")]
    Env(AgentEnvArgs),
    #[command(about = "Manage which workspace MCP servers an agent uses")]
    Mcp(AgentMcpArgs),
    #[command(about = "Copy an existing agent into a new one")]
    Copy(AgentCopyArgs),
}

#[derive(Debug, Args)]
pub(super) struct AgentCopyArgs {
    #[arg(value_name = "SOURCE-AGENT-ID")]
    pub(super) source_agent_id: String,
    #[arg(long, help = "Name for the new agent")]
    pub(super) name: Option<String>,
    #[arg(long, help = "Target runtime ID")]
    pub(super) runtime_id: Option<String>,
    #[arg(long, help = "Override the copied description")]
    pub(super) description: Option<String>,
    #[arg(long, help = "Override the copied instructions")]
    pub(super) instructions: Option<String>,
    #[arg(long, help = "Model identifier for the copy")]
    pub(super) model: Option<String>,
    #[arg(long, help = "Override thinking level")]
    pub(super) thinking_level: Option<String>,
    #[arg(long, help = "Override Codex service tier")]
    pub(super) service_tier: Option<String>,
    #[arg(long, help = "Override custom CLI arguments as a JSON array")]
    pub(super) custom_args: Option<String>,
    #[arg(long, help = "Override maximum concurrent tasks")]
    pub(super) max_concurrent_tasks: Option<i32>,
    #[arg(long, help = "Override visibility: private or workspace")]
    pub(super) visibility: Option<String>,
    #[arg(long, help = "Override invocation permission mode")]
    pub(super) permission_mode: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Allow every workspace member to invoke the copy")]
    pub(super) public_to_workspace: Option<bool>,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Allow a workspace member ID to invoke the copy")]
    pub(super) public_to_member: Vec<String>,
    #[arg(long, help = "Do not copy workspace skill assignments")]
    pub(super) no_skills: bool,
    #[arg(long, help = "Set custom_env on the copy as a JSON object")]
    pub(super) custom_env: Option<String>,
    #[arg(long, help = "Read custom_env from stdin")]
    pub(super) custom_env_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read custom_env from a file")]
    pub(super) custom_env_file: Option<PathBuf>,
    #[arg(long, help = "Set mcp_config on the copy as a JSON object")]
    pub(super) mcp_config: Option<String>,
    #[arg(long, help = "Read mcp_config from stdin")]
    pub(super) mcp_config_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read mcp_config from a file")]
    pub(super) mcp_config_file: Option<PathBuf>,
    #[arg(long, help = "Set runtime_config on the copy as JSON")]
    pub(super) runtime_config: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AgentMcpArgs {
    #[command(subcommand)]
    pub(super) command: AgentMcpCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AgentMcpCommand {
    #[command(about = "List workspace MCP servers assigned to an agent")]
    List(AgentMcpListArgs),
    #[command(about = "Give a workspace MCP server to an agent")]
    Add(AgentMcpMutationArgs),
    #[command(about = "Turn an assigned MCP server back on for this agent")]
    Enable(AgentMcpMutationArgs),
    #[command(about = "Turn an assigned MCP server off for this agent")]
    Disable(AgentMcpMutationArgs),
    #[command(about = "Take a workspace MCP server away from an agent")]
    Remove(AgentMcpMutationArgs),
}

#[derive(Debug, Args)]
pub(super) struct AgentMcpListArgs {
    #[arg(value_name = "AGENT-ID")]
    pub(super) agent_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AgentMcpMutationArgs {
    #[arg(value_name = "AGENT-ID")]
    pub(super) agent_id: String,
    #[arg(value_name = "SERVER-ID")]
    pub(super) server_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AgentEnvArgs {
    #[command(subcommand)]
    pub(super) command: AgentEnvCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AgentEnvCommand {
    #[command(about = "Print an agent's custom_env as a JSON map")]
    Get {
        #[arg(value_name = "AGENT-ID")]
        agent_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Replace an agent's custom_env")]
    Set(AgentEnvSetArgs),
}

#[derive(Debug, Args)]
pub(super) struct AgentEnvSetArgs {
    #[arg(value_name = "AGENT-ID")]
    pub(super) agent_id: String,
    #[arg(long, help = "Replacement custom_env as a JSON object")]
    pub(super) custom_env: Option<String>,
    #[arg(long, help = "Read the replacement custom_env JSON object from stdin")]
    pub(super) custom_env_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the replacement custom_env JSON object from a file"
    )]
    pub(super) custom_env_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AgentSkillsArgs {
    #[command(subcommand)]
    pub(super) command: AgentSkillsCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AgentSkillsCommand {
    #[command(about = "List skills assigned to an agent")]
    List {
        #[arg(value_name = "AGENT-ID")]
        agent_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Set skills for an agent (replaces all current assignments)")]
    Set(AgentSkillsMutationArgs),
    #[command(about = "Add skills to an agent without replacing existing assignments")]
    Add(AgentSkillsMutationArgs),
}

#[derive(Debug, Args)]
pub(super) struct AgentSkillsMutationArgs {
    #[arg(value_name = "AGENT-ID")]
    pub(super) agent_id: String,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Skill IDs to assign (comma-separated)")]
    pub(super) skill_ids: Option<Vec<String>>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AgentCreateArgs {
    #[arg(long, help = "Agent name (required)")]
    pub(super) name: Option<String>,
    #[arg(long, default_value = "", help = "Agent description")]
    pub(super) description: String,
    #[arg(long, default_value = "", help = "Agent instructions")]
    pub(super) instructions: String,
    #[arg(long, help = "Runtime ID (required)")]
    pub(super) runtime_id: Option<String>,
    #[arg(long, help = "Runtime config as JSON string")]
    pub(super) runtime_config: Option<String>,
    #[arg(long, help = "Model identifier")]
    pub(super) model: Option<String>,
    #[arg(long, help = "Reasoning/effort level for the agent runtime")]
    pub(super) thinking_level: Option<String>,
    #[arg(long, help = "Codex execution service tier")]
    pub(super) service_tier: Option<String>,
    #[arg(long, help = "Custom CLI arguments as a JSON array")]
    pub(super) custom_args: Option<String>,
    #[arg(long, help = "Custom environment variables as a JSON object")]
    pub(super) custom_env: Option<String>,
    #[arg(long, help = "Read custom environment variables from stdin")]
    pub(super) custom_env_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read custom environment variables from a file"
    )]
    pub(super) custom_env_file: Option<PathBuf>,
    #[arg(long, help = "MCP server configuration as a JSON object")]
    pub(super) mcp_config: Option<String>,
    #[arg(long, help = "Read MCP server configuration from stdin")]
    pub(super) mcp_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read MCP server configuration from a file"
    )]
    pub(super) mcp_config_file: Option<PathBuf>,
    #[arg(long, help = "Visibility: private or workspace")]
    pub(super) visibility: Option<String>,
    #[arg(long, help = "Invocation permission mode: private or public_to")]
    pub(super) permission_mode: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Allow every workspace member to invoke this agent")]
    pub(super) public_to_workspace: Option<bool>,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Allow a workspace member ID to invoke this agent (repeatable)")]
    pub(super) public_to_member: Vec<String>,
    #[arg(long, help = "Maximum concurrent tasks (1-50)")]
    pub(super) max_concurrent_tasks: Option<i32>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct AgentUpdateArgs {
    #[arg(value_name = "ID")]
    pub(super) id: String,
    #[arg(long, help = "New name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "New description")]
    pub(super) description: Option<String>,
    #[arg(long, help = "New instructions")]
    pub(super) instructions: Option<String>,
    #[arg(long, help = "New runtime ID")]
    pub(super) runtime_id: Option<String>,
    #[arg(long, help = "New runtime config as JSON string")]
    pub(super) runtime_config: Option<String>,
    #[arg(
        long,
        help = "New model identifier; empty clears to the runtime default"
    )]
    pub(super) model: Option<String>,
    #[arg(
        long,
        help = "New reasoning/effort level; empty clears to the runtime default"
    )]
    pub(super) thinking_level: Option<String>,
    #[arg(
        long,
        help = "New Codex execution service tier; empty inherits local config"
    )]
    pub(super) service_tier: Option<String>,
    #[arg(long, help = "New custom CLI arguments as a JSON array")]
    pub(super) custom_args: Option<String>,
    #[arg(long, help = "New MCP server configuration; pass null to clear")]
    pub(super) mcp_config: Option<String>,
    #[arg(long, help = "Read the new MCP server configuration from stdin")]
    pub(super) mcp_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the new MCP server configuration from a file"
    )]
    pub(super) mcp_config_file: Option<PathBuf>,
    #[arg(long, help = "New visibility: private or workspace")]
    pub(super) visibility: Option<String>,
    #[arg(long, help = "New invocation permission mode: private or public_to")]
    pub(super) permission_mode: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Allow every workspace member to invoke this agent")]
    pub(super) public_to_workspace: Option<bool>,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Allow a workspace member ID to invoke this agent (repeatable)")]
    pub(super) public_to_member: Vec<String>,
    #[arg(long, help = "New status")]
    pub(super) status: Option<String>,
    #[arg(long, help = "New maximum concurrent tasks (1-50)")]
    pub(super) max_concurrent_tasks: Option<i32>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}
