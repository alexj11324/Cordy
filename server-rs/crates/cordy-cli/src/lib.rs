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
use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::io::{Read, Write as IoWrite};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use url::{form_urlencoded, Url};

pub const CLIENT_VERSION: &str = env!("CORDY_BUILD_VERSION");
pub const BUILD_COMMIT: &str = env!("CORDY_BUILD_COMMIT");
pub const BUILD_DATE: &str = env!("CORDY_BUILD_DATE");
pub const BUILD_GO_VERSION: &str = env!("CORDY_BUILD_GO_VERSION");
pub const BUILD_OS: &str = env!("CORDY_BUILD_OS");
pub const BUILD_ARCH: &str = env!("CORDY_BUILD_ARCH");

/// Handles the daemon's private execution-environment helper mode before
/// normal CLI parsing or profile loading. The protocol never places task
/// configuration or gateway credentials in argv; all payload data stays on
/// the inherited stdin/stdout pipes.
pub async fn run_private_helper<I, O>(args: &[OsString], input: I, output: &mut O) -> Result<bool>
where
    I: Read,
    O: IoWrite,
{
    if args.len() != 2
        || args[1] != OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG)
    {
        return Ok(false);
    }
    cordy_daemon::execenv::isolation::run_preparation_helper(input, output).await?;
    Ok(true)
}
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
    #[command(about = "Work with agents")]
    Agent(AgentArgs),
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
    #[command(about = "Work with projects")]
    Project(ProjectArgs),
    #[command(about = "Manage workspace custom issue properties")]
    Property(PropertyArgs),
    #[command(about = "Work with the current chat conversation")]
    Chat(ChatArgs),
    #[command(about = "Work with attachments")]
    Attachment(AttachmentArgs),
    #[command(about = "Work with repositories")]
    Repo(RepoArgs),
    #[command(about = "Work with agent runtimes")]
    Runtime(RuntimeArgs),
    #[command(about = "Manage autopilots (scheduled/triggered agent automations)")]
    Autopilot(AutopilotArgs),
    #[command(about = "Print version information")]
    Version {
        #[arg(long, value_enum, default_value_t = VersionOutput::Text)]
        output: VersionOutput,
    },
}

#[derive(Debug, Args)]
struct AutopilotArgs {
    #[command(subcommand)]
    command: AutopilotCommand,
}

#[derive(Debug, Subcommand)]
enum AutopilotCommand {
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
}

#[derive(Debug, Args)]
struct AutopilotTriggerAddArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    autopilot_id: String,
    #[arg(
        long,
        default_value = "schedule",
        help = "Trigger kind: schedule or webhook"
    )]
    kind: String,
    #[arg(
        long,
        default_value = "",
        help = "Cron expression (required for --kind schedule)"
    )]
    cron: String,
    #[arg(
        long,
        default_value = "",
        help = "IANA timezone (default UTC; schedule only)"
    )]
    timezone: String,
    #[arg(long, default_value = "", help = "Optional human-readable label")]
    label: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AutopilotTriggerUpdateArgs {
    #[arg(value_name = "AUTOPILOT-ID")]
    autopilot_id: String,
    #[arg(value_name = "TRIGGER-ID")]
    trigger_id: String,
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    enabled: Option<bool>,
    #[arg(long)]
    cron: Option<String>,
    #[arg(long)]
    timezone: Option<String>,
    #[arg(long)]
    label: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AutopilotCreateArgs {
    #[arg(long, help = "Autopilot title (required)")]
    title: Option<String>,
    #[arg(
        long,
        default_value = "",
        help = "Autopilot description (used as task prompt)"
    )]
    description: String,
    #[arg(long, help = "Assignee agent (name or ID) — required")]
    agent: Option<String>,
    #[arg(long, help = "Execution mode: create_issue or run_only (required)")]
    mode: Option<String>,
    #[arg(
        long,
        help = "Priority for created issues (none, low, medium, high, urgent)"
    )]
    priority: Option<String>,
    #[arg(long, default_value = "", help = "Project ID (optional)")]
    project: String,
    #[arg(
        long,
        default_value = "",
        help = "Template for issue titles (create_issue mode). Only {{date}} (UTC, YYYY-MM-DD) is interpolated; any other {{...}} token is rejected at create-time."
    )]
    issue_title_template: String,
    #[arg(
        long,
        action = clap::ArgAction::Append,
        help = "Member subscriber to notify for issues this autopilot creates (name or user ID; repeatable)"
    )]
    subscriber: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AutopilotUpdateArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long, help = "New assignee agent (name or ID)")]
    agent: Option<String>,
    #[arg(long, help = "New project ID (use empty string to clear)")]
    project: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    #[arg(long, help = "New status (active, paused)")]
    status: Option<String>,
    #[arg(long, help = "New execution mode (create_issue or run_only)")]
    mode: Option<String>,
    #[arg(
        long,
        help = "New issue title template. Only {{date}} is interpolated."
    )]
    issue_title_template: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Replace subscribers with this member (repeatable)")]
    subscriber: Vec<String>,
    #[arg(long, help = "Remove all autopilot subscribers")]
    clear_subscribers: bool,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct RuntimeArgs {
    #[command(subcommand)]
    command: RuntimeCommand,
}

#[derive(Debug, Subcommand)]
enum RuntimeCommand {
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
struct RuntimeProfileArgs {
    #[command(subcommand)]
    command: RuntimeProfileCommand,
}

#[derive(Debug, Subcommand)]
enum RuntimeProfileCommand {
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
struct RuntimeProfileCreateArgs {
    #[arg(long, help = "Supported backend the profile routes to (required)")]
    protocol_family: Option<String>,
    #[arg(long, help = "Executable the daemon resolves on PATH (required)")]
    command_name: Option<String>,
    #[arg(long, help = "Human-readable profile name (required)")]
    display_name: Option<String>,
    #[arg(long, default_value = "", help = "Optional description")]
    description: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct RuntimeProfileUpdateArgs {
    #[arg(value_name = "PROFILE-ID")]
    profile_id: String,
    #[arg(long, help = "New display name")]
    display_name: Option<String>,
    #[arg(long, help = "New command name")]
    command_name: Option<String>,
    #[arg(long, help = "New description")]
    description: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Enable or disable the profile")]
    enabled: Option<bool>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
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
struct AgentCopyArgs {
    #[arg(value_name = "SOURCE-AGENT-ID")]
    source_agent_id: String,
    #[arg(long, help = "Name for the new agent")]
    name: Option<String>,
    #[arg(long, help = "Target runtime ID")]
    runtime_id: Option<String>,
    #[arg(long, help = "Override the copied description")]
    description: Option<String>,
    #[arg(long, help = "Override the copied instructions")]
    instructions: Option<String>,
    #[arg(long, help = "Model identifier for the copy")]
    model: Option<String>,
    #[arg(long, help = "Override thinking level")]
    thinking_level: Option<String>,
    #[arg(long, help = "Override Codex service tier")]
    service_tier: Option<String>,
    #[arg(long, help = "Override custom CLI arguments as a JSON array")]
    custom_args: Option<String>,
    #[arg(long, help = "Override maximum concurrent tasks")]
    max_concurrent_tasks: Option<i32>,
    #[arg(long, help = "Override visibility: private or workspace")]
    visibility: Option<String>,
    #[arg(long, help = "Override invocation permission mode")]
    permission_mode: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Allow every workspace member to invoke the copy")]
    public_to_workspace: Option<bool>,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Allow a workspace member ID to invoke the copy")]
    public_to_member: Vec<String>,
    #[arg(long, help = "Do not copy workspace skill assignments")]
    no_skills: bool,
    #[arg(long, help = "Set custom_env on the copy as a JSON object")]
    custom_env: Option<String>,
    #[arg(long, help = "Read custom_env from stdin")]
    custom_env_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read custom_env from a file")]
    custom_env_file: Option<PathBuf>,
    #[arg(long, help = "Set mcp_config on the copy as a JSON object")]
    mcp_config: Option<String>,
    #[arg(long, help = "Read mcp_config from stdin")]
    mcp_config_stdin: bool,
    #[arg(long, value_name = "PATH", help = "Read mcp_config from a file")]
    mcp_config_file: Option<PathBuf>,
    #[arg(long, help = "Set runtime_config on the copy as JSON")]
    runtime_config: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentMcpArgs {
    #[command(subcommand)]
    command: AgentMcpCommand,
}

#[derive(Debug, Subcommand)]
enum AgentMcpCommand {
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
struct AgentMcpListArgs {
    #[arg(value_name = "AGENT-ID")]
    agent_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentMcpMutationArgs {
    #[arg(value_name = "AGENT-ID")]
    agent_id: String,
    #[arg(value_name = "SERVER-ID")]
    server_id: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentEnvArgs {
    #[command(subcommand)]
    command: AgentEnvCommand,
}

#[derive(Debug, Subcommand)]
enum AgentEnvCommand {
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
struct AgentEnvSetArgs {
    #[arg(value_name = "AGENT-ID")]
    agent_id: String,
    #[arg(long, help = "Replacement custom_env as a JSON object")]
    custom_env: Option<String>,
    #[arg(long, help = "Read the replacement custom_env JSON object from stdin")]
    custom_env_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the replacement custom_env JSON object from a file"
    )]
    custom_env_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentSkillsArgs {
    #[command(subcommand)]
    command: AgentSkillsCommand,
}

#[derive(Debug, Subcommand)]
enum AgentSkillsCommand {
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
struct AgentSkillsMutationArgs {
    #[arg(value_name = "AGENT-ID")]
    agent_id: String,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Skill IDs to assign (comma-separated)")]
    skill_ids: Option<Vec<String>>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentCreateArgs {
    #[arg(long, help = "Agent name (required)")]
    name: Option<String>,
    #[arg(long, default_value = "", help = "Agent description")]
    description: String,
    #[arg(long, default_value = "", help = "Agent instructions")]
    instructions: String,
    #[arg(long, help = "Runtime ID (required)")]
    runtime_id: Option<String>,
    #[arg(long, help = "Runtime config as JSON string")]
    runtime_config: Option<String>,
    #[arg(long, help = "Model identifier")]
    model: Option<String>,
    #[arg(long, help = "Reasoning/effort level for the agent runtime")]
    thinking_level: Option<String>,
    #[arg(long, help = "Codex execution service tier")]
    service_tier: Option<String>,
    #[arg(long, help = "Custom CLI arguments as a JSON array")]
    custom_args: Option<String>,
    #[arg(long, help = "Custom environment variables as a JSON object")]
    custom_env: Option<String>,
    #[arg(long, help = "Read custom environment variables from stdin")]
    custom_env_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read custom environment variables from a file"
    )]
    custom_env_file: Option<PathBuf>,
    #[arg(long, help = "MCP server configuration as a JSON object")]
    mcp_config: Option<String>,
    #[arg(long, help = "Read MCP server configuration from stdin")]
    mcp_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read MCP server configuration from a file"
    )]
    mcp_config_file: Option<PathBuf>,
    #[arg(long, help = "Visibility: private or workspace")]
    visibility: Option<String>,
    #[arg(long, help = "Invocation permission mode: private or public_to")]
    permission_mode: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Allow every workspace member to invoke this agent")]
    public_to_workspace: Option<bool>,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Allow a workspace member ID to invoke this agent (repeatable)")]
    public_to_member: Vec<String>,
    #[arg(long, help = "Maximum concurrent tasks (1-50)")]
    max_concurrent_tasks: Option<i32>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AgentUpdateArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long, help = "New name")]
    name: Option<String>,
    #[arg(long, help = "New description")]
    description: Option<String>,
    #[arg(long, help = "New instructions")]
    instructions: Option<String>,
    #[arg(long, help = "New runtime ID")]
    runtime_id: Option<String>,
    #[arg(long, help = "New runtime config as JSON string")]
    runtime_config: Option<String>,
    #[arg(
        long,
        help = "New model identifier; empty clears to the runtime default"
    )]
    model: Option<String>,
    #[arg(
        long,
        help = "New reasoning/effort level; empty clears to the runtime default"
    )]
    thinking_level: Option<String>,
    #[arg(
        long,
        help = "New Codex execution service tier; empty inherits local config"
    )]
    service_tier: Option<String>,
    #[arg(long, help = "New custom CLI arguments as a JSON array")]
    custom_args: Option<String>,
    #[arg(long, help = "New MCP server configuration; pass null to clear")]
    mcp_config: Option<String>,
    #[arg(long, help = "Read the new MCP server configuration from stdin")]
    mcp_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the new MCP server configuration from a file"
    )]
    mcp_config_file: Option<PathBuf>,
    #[arg(long, help = "New visibility: private or workspace")]
    visibility: Option<String>,
    #[arg(long, help = "New invocation permission mode: private or public_to")]
    permission_mode: Option<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", help = "Allow every workspace member to invoke this agent")]
    public_to_workspace: Option<bool>,
    #[arg(long, action = clap::ArgAction::Append, value_delimiter = ',', help = "Allow a workspace member ID to invoke this agent (repeatable)")]
    public_to_member: Vec<String>,
    #[arg(long, help = "New status")]
    status: Option<String>,
    #[arg(long, help = "New maximum concurrent tasks (1-50)")]
    max_concurrent_tasks: Option<i32>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct RepoArgs {
    #[command(subcommand)]
    command: RepoCommand,
}

#[derive(Debug, Subcommand)]
enum RepoCommand {
    #[command(about = "List workspace repositories")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    #[command(about = "Add repositories to the workspace registry")]
    Add(RepoMutationArgs),
    #[command(
        alias = "rm",
        about = "Remove repositories from the workspace registry"
    )]
    Remove(RepoRemoveArgs),
    #[command(about = "Check out a repository into the working directory")]
    Checkout {
        #[arg(value_name = "URL")]
        url: String,
        #[arg(
            long = "ref",
            help = "branch, tag, or commit to check out instead of the remote default branch"
        )]
        checkout_ref: Option<String>,
    },
}

#[derive(Debug, Args)]
struct RepoMutationArgs {
    #[arg(value_name = "URL")]
    urls: Vec<String>,
    #[arg(long = "url", action = clap::ArgAction::Append, help = "Repository URL (may be repeated)")]
    flag_urls: Vec<String>,
    #[arg(long, help = "Optional description; only valid when adding one URL")]
    description: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct RepoRemoveArgs {
    #[arg(value_name = "URL")]
    urls: Vec<String>,
    #[arg(long = "url", action = clap::ArgAction::Append, help = "Repository URL to remove (may be repeated)")]
    flag_urls: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct AttachmentArgs {
    #[command(subcommand)]
    command: AttachmentCommand,
}

#[derive(Debug, Subcommand)]
enum AttachmentCommand {
    #[command(about = "Download an attachment to a local file")]
    Download {
        #[arg(value_name = "ATTACHMENT-ID")]
        attachment_id: String,
        #[arg(
            short = 'o',
            long,
            default_value = ".",
            help = "Directory to save the downloaded file"
        )]
        output_dir: PathBuf,
    },
    #[command(about = "Upload a file to attach to your chat reply")]
    Upload {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        #[arg(long, help = "Chat task id to attach to (defaults to CORDY_TASK_ID)")]
        task: Option<String>,
    },
}

#[derive(Debug, Args)]
struct ChatArgs {
    #[command(subcommand)]
    command: ChatCommand,
}

#[derive(Debug, Subcommand)]
enum ChatCommand {
    #[command(about = "Overview of the channel this conversation is in (messages + thread list)")]
    History(ChatReadArgs),
    #[command(about = "Read one thread's messages (the current thread, or a specific id)")]
    Thread(ChatThreadArgs),
}

#[derive(Debug, Args)]
struct ChatReadArgs {
    #[arg(
        long,
        default_value_t = 0,
        help = "Maximum number of messages to return (the server clamps the range)"
    )]
    limit: i64,
    #[arg(
        long,
        help = "Opaque cursor (a next_cursor from a prior page) to read older messages"
    )]
    before: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct ChatThreadArgs {
    #[arg(value_name = "ID")]
    id: Option<String>,
    #[command(flatten)]
    read: ChatReadArgs,
}

#[derive(Debug, Args)]
struct PropertyArgs {
    #[command(subcommand)]
    command: PropertyCommand,
}

#[derive(Debug, Subcommand)]
enum PropertyCommand {
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
struct PropertyCreateArgs {
    #[arg(long, help = "Property name (required)")]
    name: Option<String>,
    #[arg(
        long = "type",
        help = "Property type: text, number, select, multi_select, date, checkbox, url, actor, multi_actor (required)"
    )]
    property_type: Option<String>,
    #[arg(long, default_value = "", help = "Property description")]
    description: String,
    #[arg(
        long,
        default_value = "",
        help = "Property icon key from the Web picker (for example, flag, tag, or shield)"
    )]
    icon: String,
    #[arg(long, action = clap::ArgAction::Append, help = "Select option as \"Name\" or \"Name:#rrggbb\" (repeatable; select types only)")]
    option: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct PropertyUpdateArgs {
    #[arg(value_name = "ID-OR-NAME")]
    property: String,
    #[arg(long, help = "New property name")]
    name: Option<String>,
    #[arg(long, help = "New property description")]
    description: Option<String>,
    #[arg(
        long,
        help = "New property icon key from the Web picker; pass an empty value to clear"
    )]
    icon: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Replacement option list as \"Name\" or \"Name:#rrggbb\" (repeatable)")]
    option: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct PropertyArchiveArgs {
    #[arg(value_name = "ID-OR-NAME")]
    property: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
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

#[derive(Debug, Args)]
struct ProjectArgs {
    #[command(subcommand)]
    command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
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
struct ProjectResourceArgs {
    #[command(subcommand)]
    command: ProjectResourceCommand,
}

#[derive(Debug, Subcommand)]
enum ProjectResourceCommand {
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
struct ProjectResourceAddArgs {
    #[arg(value_name = "PROJECT-ID")]
    project_id: String,
    #[arg(
        long = "type",
        default_value = "github_repo",
        help = "Resource type (e.g. github_repo, local_directory — see docs)"
    )]
    resource_type: String,
    #[arg(
        long,
        help = "Shortcut: the repo URL (only used when --type github_repo)"
    )]
    url: Option<String>,
    #[arg(
        long,
        help = "Shortcut: optional default branch hint (only used when --type github_repo)"
    )]
    default_branch_hint: Option<String>,
    #[arg(
        long,
        help = "Shortcut: absolute path to the working directory (only used when --type local_directory)"
    )]
    local_path: Option<String>,
    #[arg(
        long,
        help = "Shortcut: id of the daemon that owns the local path (only used when --type local_directory)"
    )]
    daemon_id: Option<String>,
    #[arg(
        long,
        help = "Shortcut: optional label embedded in resource_ref (only used when --type local_directory)"
    )]
    ref_label: Option<String>,
    #[arg(
        long,
        help = "Shortcut: how tasks share the directory — in_place (default, one task at a time) or worktree (each task gets its own git worktree; requires a git repo) (only used when --type local_directory)"
    )]
    execution_mode: Option<String>,
    #[arg(
        long = "ref",
        help = "Generic JSON resource_ref payload, or a github_repo checkout ref when used with --url"
    )]
    resource_ref: Option<String>,
    #[arg(long, help = "Optional human-readable label")]
    label: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct ProjectResourceUpdateArgs {
    #[arg(value_name = "PROJECT-ID")]
    project_id: String,
    #[arg(value_name = "RESOURCE-ID")]
    resource_id: String,
    #[arg(long, help = "Shortcut: new repo URL (github_repo)")]
    url: Option<String>,
    #[arg(long, help = "Shortcut: new default branch hint (github_repo)")]
    default_branch_hint: Option<String>,
    #[arg(long, help = "Shortcut: new absolute local path (local_directory)")]
    local_path: Option<String>,
    #[arg(long, help = "Shortcut: new daemon id (local_directory)")]
    daemon_id: Option<String>,
    #[arg(
        long,
        help = "Shortcut: new label embedded in resource_ref (local_directory)"
    )]
    ref_label: Option<String>,
    #[arg(
        long,
        help = "Shortcut: new execution mode — in_place or worktree (local_directory)"
    )]
    execution_mode: Option<String>,
    #[arg(
        long = "ref",
        help = "Generic JSON resource_ref payload, or a github_repo checkout ref"
    )]
    resource_ref: Option<String>,
    #[arg(long, help = "New human-readable label; pass an empty string to clear")]
    label: Option<String>,
    #[arg(long, help = "Clear the human-readable label")]
    clear_label: bool,
    #[arg(long, help = "New display position")]
    position: Option<i32>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct ProjectCreateArgs {
    #[arg(long, help = "Project title (required)")]
    title: Option<String>,
    #[arg(long, help = "Project description")]
    description: Option<String>,
    #[arg(long, help = "Project status")]
    status: Option<String>,
    #[arg(long, help = "Project icon (emoji)")]
    icon: Option<String>,
    #[arg(long, help = "Lead name (member or agent)")]
    lead: Option<String>,
    #[arg(long, help = "Start date (calendar day, YYYY-MM-DD)")]
    start_date: Option<String>,
    #[arg(long, help = "Due date (calendar day, YYYY-MM-DD)")]
    due_date: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Attach a github_repo resource by URL")]
    repo: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct ProjectUpdateArgs {
    #[arg(value_name = "ID")]
    id: String,
    #[arg(long, help = "New title")]
    title: Option<String>,
    #[arg(long, help = "New description")]
    description: Option<String>,
    #[arg(long, help = "New status")]
    status: Option<String>,
    #[arg(long, help = "New icon (emoji)")]
    icon: Option<String>,
    #[arg(long, help = "New lead name (member or agent)")]
    lead: Option<String>,
    #[arg(long, help = "New start date; pass an empty string to clear")]
    start_date: Option<String>,
    #[arg(long, help = "New due date; pass an empty string to clear")]
    due_date: Option<String>,
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
        allow_hyphen_values = true,
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
struct WorkspaceMcpArgs {
    #[command(subcommand)]
    command: WorkspaceMcpCommand,
}

#[derive(Debug, Subcommand)]
enum WorkspaceMcpCommand {
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
struct WorkspaceMcpAddArgs {
    #[arg(value_name = "SERVER-NAME")]
    server_name: String,
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    workspace: Option<String>,
    #[arg(long, help = "Server entry as JSON (avoid: lands in shell history)")]
    server_config: Option<String>,
    #[arg(long, help = "Read the server entry JSON from stdin")]
    server_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the server entry JSON from a file"
    )]
    server_config_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct WorkspaceMcpUpdateArgs {
    #[arg(value_name = "SERVER-ID")]
    server_id: String,
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    workspace: Option<String>,
    #[arg(long, help = "New server name")]
    name: Option<String>,
    #[arg(
        long,
        help = "Replacement server entry as JSON (avoid: lands in shell history)"
    )]
    server_config: Option<String>,
    #[arg(long, help = "Read the replacement server entry JSON from stdin")]
    server_config_stdin: bool,
    #[arg(
        long,
        value_name = "PATH",
        help = "Read the replacement server entry JSON from a file"
    )]
    server_config_file: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct WorkspaceMemberArgs {
    #[command(subcommand)]
    command: WorkspaceMemberCommand,
}

#[derive(Debug, Subcommand)]
enum WorkspaceMemberCommand {
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
struct WorkspaceMemberInviteArgs {
    #[arg(value_name = "EMAIL")]
    email: String,
    #[arg(value_name = "WORKSPACE-ID|SLUG|PREFIX")]
    workspace: Option<String>,
    #[arg(
        long,
        default_value = "member",
        help = "Member role to grant: member or admin (owner is not allowed)"
    )]
    role: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    output: OutputFormat,
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
        Command::Agent(AgentArgs {
            command:
                AgentCommand::List {
                    output,
                    include_archived,
                },
        }) => run_agent_list(cli, environment, *output, *include_archived).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Get { id, output },
        }) => run_agent_get(cli, environment, id, *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Create(args),
        }) => run_agent_create(cli, environment, args, input).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Update(args),
        }) => run_agent_update(cli, environment, args, input).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Archive { id, output },
        }) => run_agent_lifecycle(cli, environment, id, "archive", "archived", *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Restore { id, output },
        }) => run_agent_lifecycle(cli, environment, id, "restore", "restored", *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Tasks { id, output },
        }) => run_agent_tasks(cli, environment, id, *output).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Avatar { id, file, output },
        }) => run_agent_avatar(cli, environment, id, file.as_deref(), *output).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::List { agent_id, output },
                }),
        }) => run_agent_skills_list(cli, environment, agent_id, *output).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::Set(args),
                }),
        }) => run_agent_skills_mutation(cli, environment, args, false).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Skills(AgentSkillsArgs {
                    command: AgentSkillsCommand::Add(args),
                }),
        }) => run_agent_skills_mutation(cli, environment, args, true).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Env(AgentEnvArgs {
                    command: AgentEnvCommand::Get { agent_id, output },
                }),
        }) => run_agent_env_get(cli, environment, agent_id, *output).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Env(AgentEnvArgs {
                    command: AgentEnvCommand::Set(args),
                }),
        }) => run_agent_env_set(cli, environment, args, input).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::List(args),
                }),
        }) => run_agent_mcp_list(cli, environment, args).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Add(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Add).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Enable(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Enable).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Disable(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Disable).await,
        Command::Agent(AgentArgs {
            command:
                AgentCommand::Mcp(AgentMcpArgs {
                    command: AgentMcpCommand::Remove(args),
                }),
        }) => run_agent_mcp_mutation(cli, environment, args, AgentMcpAction::Remove).await,
        Command::Agent(AgentArgs {
            command: AgentCommand::Copy(args),
        }) => run_agent_copy(cli, environment, args, input).await,
        Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::List {
                    status,
                    output,
                    full_id,
                },
        }) => run_autopilot_list(cli, environment, status, *output, *full_id).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Get { id, output },
        }) => run_autopilot_get(cli, environment, id, *output).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Create(args),
        }) => run_autopilot_create(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Update(args),
        }) => run_autopilot_update(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Delete { id },
        }) => run_autopilot_delete(cli, environment, id).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Trigger { id, output },
        }) => run_autopilot_trigger(cli, environment, id, *output).await,
        Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::Runs {
                    id,
                    limit,
                    offset,
                    output,
                },
        }) => run_autopilot_runs(cli, environment, id, *limit, *offset, *output).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerAdd(args),
        }) => run_autopilot_trigger_add(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerUpdate(args),
        }) => run_autopilot_trigger_update(cli, environment, args).await,
        Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::TriggerDelete {
                    autopilot_id,
                    trigger_id,
                },
        }) => run_autopilot_trigger_delete(cli, environment, autopilot_id, trigger_id).await,
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
        Command::Workspace(WorkspaceArgs {
            command: WorkspaceCommand::Switch { workspace },
        }) => run_workspace_switch(cli, environment, workspace).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::List { workspace, output },
                }),
        }) => run_workspace_member_list(cli, environment, workspace.as_deref(), *output).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::Invite(args),
                }),
        }) => run_workspace_member_invite(cli, environment, args).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::List { workspace, output },
                }),
        }) => run_workspace_mcp_list(cli, environment, workspace.as_deref(), *output).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::Add(args),
                }),
        }) => run_workspace_mcp_add(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command: WorkspaceMcpCommand::Update(args),
                }),
        }) => run_workspace_mcp_update(cli, environment, args, input).await,
        Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Mcp(WorkspaceMcpArgs {
                    command:
                        WorkspaceMcpCommand::Remove {
                            server_id,
                            workspace,
                            output,
                        },
                }),
        }) => {
            run_workspace_mcp_remove(cli, environment, server_id, workspace.as_deref(), *output)
                .await
        }
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
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::List {
                    output,
                    full_id,
                    status,
                },
        }) => run_project_list(cli, environment, *output, *full_id, status.as_deref()).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Get { id, output },
        }) => run_project_get(cli, environment, id, *output).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Create(args),
        }) => run_project_create(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Update(args),
        }) => run_project_update(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Delete { id, output },
        }) => run_project_delete(cli, environment, id, *output).await,
        Command::Project(ProjectArgs {
            command: ProjectCommand::Status { id, status, output },
        }) => run_project_status(cli, environment, id, status, *output).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command:
                        ProjectResourceCommand::List {
                            project_id,
                            output,
                            full_id,
                        },
                }),
        }) => run_project_resource_list(cli, environment, project_id, *output, *full_id).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        }) => run_project_resource_add(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Update(args),
                }),
        }) => run_project_resource_update(cli, environment, args).await,
        Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command:
                        ProjectResourceCommand::Remove {
                            project_id,
                            resource_id,
                            output,
                        },
                }),
        }) => run_project_resource_remove(cli, environment, project_id, resource_id, *output).await,
        Command::Property(PropertyArgs {
            command:
                PropertyCommand::List {
                    output,
                    include_archived,
                },
        }) => run_property_list(cli, environment, *output, *include_archived).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Get { property, output },
        }) => run_property_get(cli, environment, property, *output).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Create(args),
        }) => run_property_create(cli, environment, args).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Update(args),
        }) => run_property_update(cli, environment, args).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Archive(args),
        }) => run_property_archive(cli, environment, args, true).await,
        Command::Property(PropertyArgs {
            command: PropertyCommand::Unarchive(args),
        }) => run_property_archive(cli, environment, args, false).await,
        Command::Chat(ChatArgs {
            command: ChatCommand::History(args),
        }) => run_chat_read(cli, environment, "/api/chat/history", None, args, true).await,
        Command::Chat(ChatArgs {
            command: ChatCommand::Thread(args),
        }) => {
            run_chat_read(
                cli,
                environment,
                "/api/chat/thread",
                args.id.as_deref(),
                &args.read,
                false,
            )
            .await
        }
        Command::Attachment(AttachmentArgs {
            command:
                AttachmentCommand::Download {
                    attachment_id,
                    output_dir,
                },
        }) => run_attachment_download(cli, environment, attachment_id, output_dir).await,
        Command::Attachment(AttachmentArgs {
            command: AttachmentCommand::Upload { path, task },
        }) => run_attachment_upload(cli, environment, path, task.as_deref()).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::List { output },
        }) => run_repo_list(cli, environment, *output).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Add(args),
        }) => run_repo_add(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Remove(args),
        }) => run_repo_remove(cli, environment, args).await,
        Command::Repo(RepoArgs {
            command: RepoCommand::Checkout { url, checkout_ref },
        }) => run_repo_checkout(environment, url, checkout_ref.as_deref()).await,
        Command::Runtime(RuntimeArgs {
            command: RuntimeCommand::List { output },
        }) => run_runtime_list(cli, environment, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Usage {
                    runtime_id,
                    output,
                    days,
                },
        }) => run_runtime_usage(cli, environment, runtime_id, *output, *days).await,
        Command::Runtime(RuntimeArgs {
            command: RuntimeCommand::Activity { runtime_id, output },
        }) => run_runtime_activity(cli, environment, runtime_id, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Rename {
                    runtime_id,
                    name,
                    machine,
                    output,
                },
        }) => run_runtime_rename(cli, environment, runtime_id, name, *machine, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Delete {
                    runtime_id,
                    cascade,
                    output,
                },
        }) => run_runtime_delete(cli, environment, runtime_id, *cascade, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Update {
                    runtime_id,
                    target_version,
                    output,
                    wait,
                },
        }) => {
            run_runtime_update(
                cli,
                environment,
                runtime_id,
                target_version.as_deref(),
                *output,
                *wait,
            )
            .await
        }
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::List { output },
                }),
        }) => run_runtime_profile_list(cli, environment, *output).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Create(args),
                }),
        }) => run_runtime_profile_create(cli, environment, args).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Update(args),
                }),
        }) => run_runtime_profile_update(cli, environment, args).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::Delete { profile_id },
                }),
        }) => run_runtime_profile_delete(cli, environment, profile_id).await,
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::SetPath { profile_id, path },
                }),
        }) => run_runtime_profile_set_path(cli, environment, profile_id, path.as_deref()),
        Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Profile(RuntimeProfileArgs {
                    command: RuntimeProfileCommand::UnsetPath { profile_id },
                }),
        }) => run_runtime_profile_unset_path(cli, environment, profile_id),
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

async fn run_runtime_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let runtimes: Vec<Value> = client
        .get_json("/api/runtimes")
        .await
        .context("list runtimes")?;
    Ok(RunOutput {
        stdout: format_runtime_rows(
            &runtimes,
            output,
            &["ID", "NAME", "MODE", "PROVIDER", "STATUS", "LAST_SEEN"],
            &[
                "id",
                "name",
                "runtime_mode",
                "provider",
                "status",
                "last_seen_at",
            ],
        )?,
        stderr: String::new(),
    })
}

async fn run_runtime_usage(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    output: OutputFormat,
    days: i32,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    if !(1..=365).contains(&days) {
        bail!("--days must be between 1 and 365");
    }
    let usage: Vec<Value> = client
        .get_json(&format!("/api/runtimes/{runtime_id}/usage?days={days}"))
        .await
        .context("get runtime usage")?;
    Ok(RunOutput {
        stdout: format_runtime_rows(
            &usage,
            output,
            &[
                "DATE",
                "PROVIDER",
                "MODEL",
                "INPUT_TOKENS",
                "OUTPUT_TOKENS",
                "CACHE_READ",
                "CACHE_WRITE",
            ],
            &[
                "date",
                "provider",
                "model",
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
            ],
        )?,
        stderr: String::new(),
    })
}

async fn run_runtime_activity(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let activity: Vec<Value> = client
        .get_json(&format!("/api/runtimes/{runtime_id}/activity"))
        .await
        .context("get runtime activity")?;
    Ok(RunOutput {
        stdout: format_runtime_rows(&activity, output, &["HOUR", "COUNT"], &["hour", "count"])?,
        stderr: String::new(),
    })
}

async fn run_runtime_rename(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    name: &str,
    machine: bool,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let mut body = serde_json::Map::from_iter([("custom_name".into(), Value::String(name.into()))]);
    if machine {
        body.insert("apply_to_machine".into(), Value::Bool(true));
    }
    let runtime: Value = client
        .patch_json(&format!("/api/runtimes/{runtime_id}"), &body)
        .await
        .context("rename runtime")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&runtime)?),
            OutputFormat::Table => String::new(),
        },
        stderr: match output {
            OutputFormat::Json => String::new(),
            OutputFormat::Table if name.trim().is_empty() => format!(
                "Custom name cleared; runtime is now {:?}.\n",
                value_string(&runtime, "name")
            ),
            OutputFormat::Table => format!(
                "Runtime renamed to {:?}.\n",
                value_string(&runtime, "custom_name")
            ),
        },
    })
}

#[derive(Debug, Deserialize)]
struct RuntimeDeleteConflict {
    code: String,
    #[serde(default)]
    active_agents: Vec<RuntimeDeleteAgent>,
}

#[derive(Debug, Deserialize)]
struct RuntimeDeleteAgent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

impl RuntimeDeleteConflict {
    fn ids(&self) -> Vec<&str> {
        self.active_agents
            .iter()
            .map(|agent| agent.id.as_str())
            .filter(|id| !id.is_empty())
            .collect()
    }

    fn displays(&self) -> Vec<String> {
        self.active_agents
            .iter()
            .filter_map(|agent| match (agent.name.is_empty(), agent.id.is_empty()) {
                (false, false) => Some(format!("{} ({})", agent.name, agent.id)),
                (false, true) => Some(agent.name.clone()),
                (true, false) => Some(agent.id.clone()),
                (true, true) => None,
            })
            .collect()
    }
}

fn runtime_delete_conflict(error: &anyhow::Error) -> Option<RuntimeDeleteConflict> {
    let http = error.downcast_ref::<HttpError>()?;
    if http.status_code != 409 {
        return None;
    }
    let conflict: RuntimeDeleteConflict = serde_json::from_str(&http.body).ok()?;
    (conflict.code == "runtime_has_active_agents" && !conflict.active_agents.is_empty())
        .then_some(conflict)
}

async fn run_runtime_delete(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    cascade: bool,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let mut result = match client.delete(&format!("/api/runtimes/{runtime_id}")).await {
        Ok(()) => serde_json::Map::new(),
        Err(error) => {
            let Some(conflict) = runtime_delete_conflict(&error) else {
                return Err(error).context("delete runtime");
            };
            if !cascade {
                bail!(
                    "delete runtime: runtime has active agents bound to it ({}); rebind them to another runtime first, or rerun with --cascade to unbind them and delete the runtime (the agents and their history are kept)",
                    conflict.displays().join(", ")
                );
            }
            let response: Value = client
                .post_json(
                    &format!("/api/runtimes/{runtime_id}/unbind-agents-and-delete"),
                    &serde_json::json!({"expected_active_agent_ids":conflict.ids()}),
                )
                .await
                .context("cascade delete runtime")?;
            response
                .as_object()
                .cloned()
                .context("cascade delete runtime response must be a JSON object")?
        }
    };
    result.insert("id".into(), Value::String(runtime_id.into()));
    result.insert("deleted".into(), Value::Bool(true));
    format_runtime_delete_result(&Value::Object(result), output)
}

fn format_runtime_delete_result(result: &Value, output: OutputFormat) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(result)?),
            stderr: String::new(),
        });
    }
    let id = value_string(result, "id");
    let stderr = if result.get("agents_unbound").is_some() {
        let mut message = format!(
            "Runtime {id} deleted; unbound {} agent(s)",
            value_string(result, "agents_unbound")
        );
        if result.get("autopilots_paused").is_some() {
            let _ = write!(
                message,
                " and paused {} autopilot(s)",
                value_string(result, "autopilots_paused")
            );
        }
        message + ".\n"
    } else if result.get("agents_archived").is_some() {
        format!(
            "Runtime {id} deleted; processed {} agent(s).\n",
            value_string(result, "agents_archived")
        )
    } else {
        format!("Runtime {id} deleted.\n")
    };
    Ok(RunOutput {
        stdout: String::new(),
        stderr,
    })
}

async fn run_runtime_update(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    target_version: Option<&str>,
    output: OutputFormat,
    wait: bool,
) -> Result<RunOutput> {
    run_runtime_update_with_policy(
        cli,
        environment,
        runtime_id,
        target_version,
        output,
        wait,
        Duration::from_secs(2),
        Duration::from_secs(150),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_runtime_update_with_policy(
    cli: &Cli,
    environment: &Environment,
    runtime_id: &str,
    target_version: Option<&str>,
    output: OutputFormat,
    wait: bool,
    poll_interval: Duration,
    max_wait: Duration,
) -> Result<RunOutput> {
    let request_timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(max_wait);
    let client = new_api_client(cli, environment)?.with_request_timeout(request_timeout);
    let target_version = target_version
        .filter(|version| !version.is_empty())
        .context("--target-version is required")?;
    let started = Instant::now();
    let mut update: Value = client
        .post_json(
            &format!("/api/runtimes/{runtime_id}/update"),
            &serde_json::json!({"target_version":target_version}),
        )
        .await
        .context("initiate update")?;
    if !wait {
        return format_runtime_update_result(&update, output, false);
    }
    let update_id = value_string(&update, "id");
    let remaining = max_wait.saturating_sub(started.elapsed());
    let poll = async {
        loop {
            tokio::time::sleep(poll_interval).await;
            update = client
                .get_json(&format!("/api/runtimes/{runtime_id}/update/{update_id}"))
                .await
                .context("get update status")?;
            if matches!(
                value_string(&update, "status").as_str(),
                "completed" | "failed" | "timeout"
            ) {
                return Ok::<Value, anyhow::Error>(update.clone());
            }
        }
    };
    match tokio::time::timeout(remaining, poll).await {
        Ok(Ok(final_update)) => format_runtime_update_result(&final_update, output, true),
        Ok(Err(error)) => Err(error),
        Err(_) => bail!(
            "timed out waiting for update (last status: {})",
            value_string(&update, "status")
        ),
    }
}

fn format_runtime_update_result(
    update: &Value,
    output: OutputFormat,
    waited: bool,
) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(update)?),
        OutputFormat::Table if !waited => format!(
            "Update initiated: {} (status: {})\n",
            value_string(update, "id"),
            value_string(update, "status")
        ),
        OutputFormat::Table if value_string(update, "status") == "completed" => {
            format!("Update completed: {}\n", value_string(update, "output"))
        }
        OutputFormat::Table => format!(
            "Update {}: {}\n",
            value_string(update, "status"),
            value_string(update, "error")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

const RUNTIME_PROTOCOL_FAMILIES: &[&str] = &[
    "claude",
    "codebuddy",
    "codex",
    "copilot",
    "opencode",
    "deveco",
    "openclaw",
    "hermes",
    "pi",
    "cursor",
    "kimi",
    "reasonix",
    "dsh",
    "kiro",
    "antigravity",
    "qoder",
    "qoderclicn",
    "traecli",
    "grok",
    "qwen",
    "qwenpaw",
    "mcode",
    "dim",
];

#[derive(Debug, Deserialize)]
struct RuntimeProfileListResponse {
    #[serde(default)]
    runtime_profiles: Vec<Value>,
}

fn runtime_profiles_path(workspace_id: &str) -> String {
    format!("/api/workspaces/{workspace_id}/runtime-profiles")
}

async fn run_runtime_profile_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let response: RuntimeProfileListResponse = client
        .get_json(&runtime_profiles_path(&workspace_id))
        .await
        .context("list runtime profiles")?;
    output_runtime_profiles(&response.runtime_profiles, output, false)
}

async fn run_runtime_profile_create(
    cli: &Cli,
    environment: &Environment,
    args: &RuntimeProfileCreateArgs,
) -> Result<RunOutput> {
    let family = args
        .protocol_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("--protocol-family is required")?;
    let command_name = args
        .command_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("--command-name is required")?;
    let display_name = args
        .display_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("--display-name is required")?;
    if !RUNTIME_PROTOCOL_FAMILIES.contains(&family) {
        bail!(
            "invalid --protocol-family {:?}: must be one of {}",
            family,
            RUNTIME_PROTOCOL_FAMILIES.join(", ")
        );
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let mut body = serde_json::Map::from_iter([
        ("display_name".into(), Value::String(display_name.into())),
        ("protocol_family".into(), Value::String(family.into())),
        ("command_name".into(), Value::String(command_name.into())),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    let profile: Value = client
        .post_json(&runtime_profiles_path(&workspace_id), &body)
        .await
        .context("create runtime profile")?;
    output_runtime_profiles(&[profile], args.output, true)
}

async fn run_runtime_profile_update(
    cli: &Cli,
    environment: &Environment,
    args: &RuntimeProfileUpdateArgs,
) -> Result<RunOutput> {
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("display_name", &args.display_name),
        ("command_name", &args.command_name),
        ("description", &args.description),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(enabled) = args.enabled {
        body.insert("enabled".into(), Value::Bool(enabled));
    }
    if body.is_empty() {
        bail!("no fields to update: pass at least one of --display-name, --command-name, --description, --enabled");
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let profile: Value = client
        .patch_json(
            &format!(
                "{}/{}",
                runtime_profiles_path(&workspace_id),
                args.profile_id
            ),
            &body,
        )
        .await
        .context("update runtime profile")?;
    output_runtime_profiles(&[profile], args.output, true)
}

async fn run_runtime_profile_delete(
    cli: &Cli,
    environment: &Environment,
    profile_id: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let path = format!("{}/{profile_id}", runtime_profiles_path(&workspace_id));
    if let Err(error) = client.delete(&path).await {
        if error
            .downcast_ref::<HttpError>()
            .is_some_and(|http| http.status_code == 409)
        {
            let message = error
                .downcast_ref::<HttpError>()
                .map(|http| http.body.trim())
                .filter(|body| !body.is_empty())
                .unwrap_or("profile still has active agents bound to it");
            bail!("cannot delete runtime profile {profile_id}: {message}");
        }
        return Err(error).context("delete runtime profile");
    }
    Ok(RunOutput {
        stdout: format!("Deleted runtime profile {profile_id}\n"),
        stderr: String::new(),
    })
}

fn run_runtime_profile_set_path(
    cli: &Cli,
    environment: &Environment,
    profile_id: &str,
    path: Option<&str>,
) -> Result<RunOutput> {
    require_human_local_command(environment, "runtime profile set-path")?;
    let path = path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .context("--path is required")?;
    if !Path::new(path).is_absolute() {
        bail!("--path must be an absolute path, got {path:?}");
    }
    environment
        .set_profile_command_override(&cli.profile, profile_id, Some(path))
        .context("save CLI config")?;
    Ok(RunOutput {
        stdout: format!(
            "Pinned runtime profile {profile_id} to {path} on this machine.\nRestart the daemon for the change to take effect.\n"
        ),
        stderr: String::new(),
    })
}

fn run_runtime_profile_unset_path(
    cli: &Cli,
    environment: &Environment,
    profile_id: &str,
) -> Result<RunOutput> {
    require_human_local_command(environment, "runtime profile unset-path")?;
    let changed = environment
        .set_profile_command_override(&cli.profile, profile_id, None)
        .context("save CLI config")?;
    Ok(RunOutput {
        stdout: if changed {
            format!(
                "Removed per-machine path override for runtime profile {profile_id}.\nRestart the daemon for the change to take effect.\n"
            )
        } else {
            format!("No per-machine path override set for runtime profile {profile_id}.\n")
        },
        stderr: String::new(),
    })
}

fn output_runtime_profiles(
    profiles: &[Value],
    output: OutputFormat,
    single: bool,
) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        let value = if single {
            &profiles[0]
        } else {
            return Ok(RunOutput {
                stdout: format!("{}\n", serde_json::to_string_pretty(profiles)?),
                stderr: String::new(),
            });
        };
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(value)?),
            stderr: String::new(),
        });
    }
    let mut profiles = profiles.to_vec();
    profiles.sort_by_key(|profile| value_string(profile, "display_name"));
    let mut rows = vec![vec![
        "ID".into(),
        "DISPLAY_NAME".into(),
        "PROTOCOL_FAMILY".into(),
        "COMMAND_NAME".into(),
        "ENABLED".into(),
    ]];
    rows.extend(profiles.iter().map(|profile| {
        vec![
            value_string(profile, "id"),
            value_string(profile, "display_name"),
            value_string(profile, "protocol_family"),
            value_string(profile, "command_name"),
            value_string(profile, "enabled"),
        ]
    }));
    Ok(RunOutput {
        stdout: format_table(&rows),
        stderr: String::new(),
    })
}

fn format_runtime_rows(
    values: &[Value],
    output: OutputFormat,
    headers: &[&str],
    fields: &[&str],
) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(values)?));
    }
    let mut rows = vec![headers.iter().map(|header| (*header).into()).collect()];
    rows.extend(values.iter().map(|value| {
        fields
            .iter()
            .map(|field| value_string(value, field))
            .collect()
    }));
    Ok(format_table(&rows))
}

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotListEnvelope {
    autopilots: Vec<Value>,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct AutopilotResolverEnvelope {
    autopilots: Vec<Value>,
    #[serde(default)]
    total: i64,
    #[serde(default)]
    has_more: bool,
}

async fn run_autopilot_list(
    cli: &Cli,
    environment: &Environment,
    status: &str,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let path = if status.is_empty() {
        "/api/autopilots".into()
    } else {
        format!(
            "/api/autopilots?status={}",
            form_urlencoded::byte_serialize(status.as_bytes()).collect::<String>()
        )
    };
    let response: AutopilotListEnvelope =
        client.get_json(&path).await.context("list autopilots")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let agents =
                load_autopilot_agent_names(&client, &workspace_id, &response.autopilots).await;
            format_autopilot_table(&response.autopilots, full_id, &agents)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_autopilot_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let response: Value = client
        .get_json(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .context("get autopilot")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let autopilot = response.get("autopilot").unwrap_or(&Value::Null);
            let agents =
                load_autopilot_agent_names(&client, &workspace_id, std::slice::from_ref(autopilot))
                    .await;
            format_autopilot_table(std::slice::from_ref(autopilot), true, &agents)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_autopilot_create(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotCreateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let title = args.title.as_deref().unwrap_or_default();
    if title.is_empty() {
        bail!("--title is required");
    }
    let agent = args.agent.as_deref().unwrap_or_default();
    if agent.is_empty() {
        bail!("--agent is required (agent name or ID)");
    }
    let mode = args.mode.as_deref().unwrap_or_default();
    if mode.is_empty() {
        bail!("--mode is required (create_issue or run_only)");
    }
    if !matches!(mode, "create_issue" | "run_only") {
        bail!("--mode must be create_issue or run_only");
    }

    let agent_id = resolve_autopilot_agent(&client, &workspace_id, agent)
        .await
        .map_err(|error| anyhow::anyhow!("resolve agent: {error:#}"))?;
    let mut body = serde_json::Map::from_iter([
        ("title".into(), Value::String(title.into())),
        ("assignee_id".into(), Value::String(agent_id)),
        ("execution_mode".into(), Value::String(mode.into())),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    if let Some(priority) = &args.priority {
        body.insert("priority".into(), Value::String(priority.clone()));
    }
    if !args.project.is_empty() {
        let project_id = resolve_project_reference(&client, &workspace_id, &args.project)
            .await
            .map(|(id, _)| id)
            .map_err(|error| anyhow::anyhow!("resolve project: {error:#}"))?;
        body.insert("project_id".into(), Value::String(project_id));
    }
    if !args.issue_title_template.is_empty() {
        body.insert(
            "issue_title_template".into(),
            Value::String(args.issue_title_template.clone()),
        );
    }
    if !args.subscriber.is_empty() {
        body.insert(
            "subscribers".into(),
            Value::Array(
                resolve_autopilot_subscribers(&client, &workspace_id, &args.subscriber).await?,
            ),
        );
    }

    let result: Value = client
        .post_json("/api/autopilots", &body)
        .await
        .context("create autopilot")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!(
                "Autopilot created: {} ({})\n",
                value_string(&result, "title"),
                value_string(&result, "id")
            ),
        },
        stderr: String::new(),
    })
}

async fn run_autopilot_update(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;

    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("title", args.title.as_ref()),
        ("description", args.description.as_ref()),
        ("priority", args.priority.as_ref()),
        ("status", args.status.as_ref()),
        ("issue_title_template", args.issue_title_template.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(agent) = &args.agent {
        let agent_id = resolve_autopilot_agent(&client, &workspace_id, agent)
            .await
            .map_err(|error| anyhow::anyhow!("resolve agent: {error:#}"))?;
        body.insert("assignee_type".into(), Value::String("agent".into()));
        body.insert("assignee_id".into(), Value::String(agent_id));
    }
    if let Some(project) = &args.project {
        let value = if project.is_empty() {
            Value::Null
        } else {
            let id = resolve_project_reference(&client, &workspace_id, project)
                .await
                .map(|(id, _)| id)
                .map_err(|error| anyhow::anyhow!("resolve project: {error:#}"))?;
            Value::String(id)
        };
        body.insert("project_id".into(), value);
    }
    if let Some(mode) = &args.mode {
        if !matches!(mode.as_str(), "create_issue" | "run_only") {
            bail!("--mode must be create_issue or run_only");
        }
        body.insert("execution_mode".into(), Value::String(mode.clone()));
    }
    if args.clear_subscribers && !args.subscriber.is_empty() {
        bail!("--subscriber and --clear-subscribers are mutually exclusive");
    }
    if args.clear_subscribers {
        body.insert("subscribers".into(), Value::Array(Vec::new()));
    } else if !args.subscriber.is_empty() {
        body.insert(
            "subscribers".into(),
            Value::Array(
                resolve_autopilot_subscribers(&client, &workspace_id, &args.subscriber).await?,
            ),
        );
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --description, --agent, --status, --mode, etc."
        );
    }

    let result: Value = client
        .patch_json(&format!("/api/autopilots/{autopilot_id}"), &body)
        .await
        .context("update autopilot")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!(
                "Autopilot updated: {} ({})\n",
                value_string(&result, "title"),
                value_string(&result, "id")
            ),
        },
        stderr: String::new(),
    })
}

async fn run_autopilot_delete(cli: &Cli, environment: &Environment, id: &str) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, display) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    client
        .delete(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .context("delete autopilot")?;
    Ok(RunOutput {
        stdout: format!("Autopilot {display} deleted.\n"),
        stderr: String::new(),
    })
}

async fn run_autopilot_trigger(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT"))
        .saturating_add(Duration::from_secs(5))
        .max(Duration::from_secs(30));
    let client = new_api_client(cli, environment)?.with_request_timeout(timeout);
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let run: Value = client
        .post_json(
            &format!("/api/autopilots/{autopilot_id}/trigger"),
            &Value::Null,
        )
        .await
        .context("trigger autopilot")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&run)?),
            OutputFormat::Table => format!(
                "Autopilot triggered: run {} (status: {})\n",
                value_string(&run, "id"),
                value_string(&run, "status")
            ),
        },
        stderr: String::new(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotRunsEnvelope {
    runs: Vec<Value>,
    total: i64,
}

async fn run_autopilot_runs(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    limit: i32,
    offset: i32,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    if limit > 0 {
        query.append_pair("limit", &limit.to_string());
    }
    if offset > 0 {
        query.append_pair("offset", &offset.to_string());
    }
    let query = query.finish();
    let path = if query.is_empty() {
        format!("/api/autopilots/{autopilot_id}/runs")
    } else {
        format!("/api/autopilots/{autopilot_id}/runs?{query}")
    };
    let response: AutopilotRunsEnvelope = client.get_json(&path).await.context("list runs")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
            OutputFormat::Table => format_autopilot_runs_table(&response.runs),
        },
        stderr: String::new(),
    })
}

fn format_autopilot_runs_table(runs: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "SOURCE".into(),
        "STATUS".into(),
        "ISSUE".into(),
        "TRIGGERED_AT".into(),
        "COMPLETED_AT".into(),
    ]];
    rows.extend(runs.iter().map(|run| {
        vec![
            value_string(run, "id"),
            value_string(run, "source"),
            value_string(run, "status"),
            value_string(run, "issue_id"),
            value_string(run, "triggered_at"),
            value_string(run, "completed_at"),
        ]
    }));
    format_table(&rows)
}

async fn run_autopilot_trigger_add(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotTriggerAddArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let kind = if args.kind.is_empty() {
        "schedule"
    } else {
        args.kind.as_str()
    };
    if !matches!(kind, "schedule" | "webhook") {
        bail!("--kind must be schedule or webhook");
    }
    if kind == "schedule" && args.cron.is_empty() {
        bail!("--cron is required for --kind schedule");
    }
    if kind == "webhook" && !args.timezone.is_empty() {
        bail!("--timezone is only valid with --kind schedule");
    }
    if kind == "webhook" && !args.cron.is_empty() {
        bail!("--cron is only valid with --kind schedule");
    }

    let mut body = serde_json::Map::from_iter([("kind".into(), Value::String(kind.into()))]);
    if kind == "schedule" {
        body.insert("cron_expression".into(), Value::String(args.cron.clone()));
        if !args.timezone.is_empty() {
            body.insert("timezone".into(), Value::String(args.timezone.clone()));
        }
    }
    if !args.label.is_empty() {
        body.insert("label".into(), Value::String(args.label.clone()));
    }
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.autopilot_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let result: Value = client
        .post_json(&format!("/api/autopilots/{autopilot_id}/triggers"), &body)
        .await
        .context("create trigger")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => {
            let mut text = format!(
                "Trigger created: {} (kind={})\n",
                value_string(&result, "id"),
                value_string(&result, "kind")
            );
            if kind == "webhook" {
                if let Some(url) = autopilot_webhook_url(&result, client.base_url()) {
                    let _ = writeln!(text, "Webhook URL: {url}");
                }
            }
            text
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn autopilot_webhook_url(trigger: &Value, base_url: &str) -> Option<String> {
    let url = value_string(trigger, "webhook_url");
    if !url.is_empty() {
        return Some(url);
    }
    let path = value_string(trigger, "webhook_path");
    (!path.is_empty()).then(|| format!("{}{path}", base_url.trim_end_matches('/')))
}

async fn run_autopilot_trigger_update(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotTriggerUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let mut body = serde_json::Map::new();
    if let Some(enabled) = args.enabled {
        body.insert("enabled".into(), Value::Bool(enabled));
    }
    for (key, value) in [
        ("cron_expression", args.cron.as_ref()),
        ("timezone", args.timezone.as_ref()),
        ("label", args.label.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if body.is_empty() {
        bail!("no fields to update; use --enabled, --cron, --timezone, or --label");
    }
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.autopilot_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let trigger_id = resolve_autopilot_trigger_id(&client, &autopilot_id, &args.trigger_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve trigger: {error:#}"))?;
    let result: Value = client
        .patch_json(
            &format!("/api/autopilots/{autopilot_id}/triggers/{trigger_id}"),
            &body,
        )
        .await
        .context("update trigger")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => {
                format!("Trigger updated: {}\n", value_string(&result, "id"))
            }
        },
        stderr: String::new(),
    })
}

async fn run_autopilot_trigger_delete(
    cli: &Cli,
    environment: &Environment,
    autopilot: &str,
    trigger: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, autopilot)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let trigger_id = resolve_autopilot_trigger_id(&client, &autopilot_id, trigger)
        .await
        .map_err(|error| anyhow::anyhow!("resolve trigger: {error:#}"))?;
    client
        .delete(&format!(
            "/api/autopilots/{autopilot_id}/triggers/{trigger_id}"
        ))
        .await
        .context("delete trigger")?;
    Ok(RunOutput {
        stdout: format!("Trigger {trigger_id} deleted.\n"),
        stderr: String::new(),
    })
}

async fn resolve_autopilot_trigger_id(
    client: &ApiClient,
    autopilot_id: &str,
    input: &str,
) -> Result<String> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok(trimmed.into());
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("autopilot trigger id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve autopilot trigger: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve autopilot trigger: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let response: Value = client
        .get_json(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot trigger: {error:#}"))?;
    let mut matches = response
        .get("triggers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|trigger| value_string(trigger, "id"))
        .filter(|id| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!(
            "no autopilot trigger found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous autopilot trigger id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches.join("\n  ")
        ),
    }
}

async fn resolve_autopilot_agent(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<String> {
    if is_canonical_uuid(input) {
        return Ok(input.into());
    }
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required to resolve agents; use --workspace-id or set CORDY_WORKSPACE_ID"
        );
    }
    let path = format!(
        "/api/agents?workspace_id={}",
        form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
    );
    let agents: Vec<Value> = client.get_json(&path).await.context("fetch agents")?;
    let input_lower = input.to_ascii_lowercase();
    let matches = agents
        .iter()
        .filter(|agent| {
            value_string(agent, "name")
                .to_ascii_lowercase()
                .contains(&input_lower)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [agent] => Ok(value_string(agent, "id")),
        [] => bail!("no agent found matching {input:?}"),
        agents => {
            let details = agents
                .iter()
                .map(|agent| {
                    format!(
                        "  {:?} ({})",
                        value_string(agent, "name"),
                        display_id(&value_string(agent, "id"), false)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("ambiguous agent {input:?}; matches:\n{details}")
        }
    }
}

async fn resolve_autopilot_subscribers(
    client: &ApiClient,
    workspace_id: &str,
    refs: &[String],
) -> Result<Vec<Value>> {
    for raw in refs {
        if raw.trim().is_empty() {
            bail!("--subscriber cannot be empty");
        }
    }
    let path = format!("/api/workspaces/{workspace_id}/members");
    let members: Vec<Value> = retry_actor_get(client, &path).await.map_err(|error| {
        anyhow::anyhow!(
            "resolve subscriber {:?}: failed to resolve assignee: fetch members: {error:#}",
            refs.first().map(String::as_str).unwrap_or_default()
        )
    })?;
    let mut seen = HashSet::new();
    let mut subscribers = Vec::new();
    for raw in refs {
        let input = normalize_assignee_input(raw);
        let input_lower = input.to_ascii_lowercase();
        let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
        for member in &members {
            let id = value_string(member, "user_id");
            let name = value_string(member, "name");
            let email = value_string(member, "email");
            if id.eq_ignore_ascii_case(&input)
                || display_id(&id, false).eq_ignore_ascii_case(&input)
                || (!email.is_empty() && email.eq_ignore_ascii_case(&input))
            {
                buckets[0].push(member);
            } else if name.eq_ignore_ascii_case(&input) {
                buckets[1].push(member);
            } else if name.to_ascii_lowercase().contains(&input_lower) {
                buckets[2].push(member);
            }
        }
        let member = buckets
            .iter()
            .find(|bucket| !bucket.is_empty())
            .ok_or_else(|| {
                let missing = if input.is_empty() {
                    raw.as_str()
                } else {
                    input.as_str()
                };
                anyhow::anyhow!("resolve subscriber {raw:?}: no member found matching {missing:?}")
            })?;
        if member.len() > 1 {
            let details = member
                .iter()
                .map(|member| {
                    format!(
                        "  member {:?} ({})",
                        value_string(member, "name"),
                        display_id(&value_string(member, "user_id"), false)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("resolve subscriber {raw:?}: ambiguous assignee {input:?}; matches:\n{details}");
        }
        let user_id = value_string(member[0], "user_id");
        if seen.insert(user_id.clone()) {
            subscribers.push(serde_json::json!({"user_type":"member","user_id":user_id}));
        }
    }
    Ok(subscribers)
}

async fn resolve_autopilot_id(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<(String, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("autopilot id is required");
    }
    if is_canonical_uuid(trimmed) {
        return Ok((trimmed.into(), trimmed.into()));
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve autopilot: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve autopilot: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    if workspace_id.is_empty() {
        bail!("resolve autopilot: workspace_id is required to resolve autopilot id prefixes");
    }

    const LIMIT: usize = 50;
    let mut offset = 0;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    loop {
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("limit", &LIMIT.to_string());
        if offset > 0 {
            query.append_pair("offset", &offset.to_string());
        }
        query.append_pair("workspace_id", workspace_id);
        let page: AutopilotResolverEnvelope = client
            .get_json(&format!("/api/autopilots?{}", query.finish()))
            .await
            .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
        let page_len = page.autopilots.len();
        let mut added = 0;
        for autopilot in page.autopilots {
            let id = value_string(&autopilot, "id");
            if !id.is_empty() && seen.insert(id.clone()) {
                added += 1;
                let title = value_string(&autopilot, "title");
                candidates.push((id.clone(), if title.is_empty() { id } else { title }));
            }
        }
        offset += page_len;
        if page_len == 0 || added == 0 || page_len < LIMIT {
            break;
        }
        if page.has_more {
            continue;
        }
        if page.total > 0 {
            if offset as i64 >= page.total {
                break;
            }
            continue;
        }
        break;
    }

    let mut matches = candidates
        .into_iter()
        .filter(|(id, _)| compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [resolved] => Ok(resolved.clone()),
        [] => bail!(
            "no autopilot found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous autopilot id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

async fn load_autopilot_agent_names(
    client: &ApiClient,
    workspace_id: &str,
    autopilots: &[Value],
) -> HashMap<String, String> {
    if workspace_id.is_empty()
        || !autopilots
            .iter()
            .any(|autopilot| !value_string(autopilot, "assignee_id").is_empty())
    {
        return HashMap::new();
    }
    let path = format!(
        "/api/agents?workspace_id={}",
        form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
    );
    let Ok(agents) = client.get_json::<Vec<Value>>(&path).await else {
        return HashMap::new();
    };
    agents
        .into_iter()
        .filter_map(|agent| {
            let id = value_string(&agent, "id");
            let name = value_string(&agent, "name");
            (!id.is_empty() && !name.is_empty()).then_some((id, name))
        })
        .collect()
}

fn format_autopilot_table(
    autopilots: &[Value],
    full_id: bool,
    agents: &HashMap<String, String>,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TITLE".into(),
        "STATUS".into(),
        "MODE".into(),
        "ASSIGNEE".into(),
        "LAST_RUN".into(),
    ]];
    rows.extend(autopilots.iter().map(|autopilot| {
        let assignee_id = value_string(autopilot, "assignee_id");
        vec![
            display_id(&value_string(autopilot, "id"), full_id),
            value_string(autopilot, "title"),
            value_string(autopilot, "status"),
            value_string(autopilot, "execution_mode"),
            agents.get(&assignee_id).cloned().unwrap_or(assignee_id),
            value_string(autopilot, "last_run_at"),
        ]
    }));
    format_table(&rows)
}

fn chat_reply_count(message: &Value) -> String {
    message
        .get("reply_count")
        .and_then(Value::as_f64)
        .filter(|count| *count != 0.0)
        .map(|count| (count as i64).to_string())
        .unwrap_or_default()
}

async fn run_agent_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    include_archived: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("workspace_id", &workspace_id);
    if include_archived {
        query.append_pair("include_archived", "true");
    }
    let agents: Vec<Value> = client
        .get_json(&format!("/api/agents?{}", query.finish()))
        .await
        .context("list agents")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agents)?),
        OutputFormat::Table => format_agent_list_table(&agents),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let agent: Value = client
        .get_json(&format!("/api/agents/{id}"))
        .await
        .context("get agent")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agent)?),
        OutputFormat::Table => format_agent_details_table(&agent),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_create<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AgentCreateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let name = args
        .name
        .as_deref()
        .filter(|value| !value.is_empty())
        .context("--name is required")?;
    let runtime_id = args
        .runtime_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .context("--runtime-id is required")?;
    if let Some(value) = args.max_concurrent_tasks {
        if !(1..=50).contains(&value) {
            bail!("--max-concurrent-tasks must be between 1 and 50 (got {value})");
        }
    }

    let mut body = serde_json::Map::from_iter([
        ("name".into(), Value::String(name.into())),
        ("runtime_id".into(), Value::String(runtime_id.into())),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    if !args.instructions.is_empty() {
        body.insert(
            "instructions".into(),
            Value::String(args.instructions.clone()),
        );
    }
    if let Some(raw) = &args.runtime_config {
        body.insert(
            "runtime_config".into(),
            serde_json::from_str(raw).context("--runtime-config must be valid JSON")?,
        );
    }
    if let Some(raw) = &args.custom_args {
        let values: Vec<String> = serde_json::from_str(raw)
            .map_err(|_| anyhow::anyhow!("--custom-args must be a valid JSON array of strings"))?;
        body.insert("custom_args".into(), serde_json::to_value(values)?);
    }
    if let Some(value) = resolve_agent_secret_json(
        args.custom_env.as_deref(),
        args.custom_env_stdin,
        args.custom_env_file.as_deref(),
        "custom-env",
        false,
        environment,
        input,
    )? {
        validate_agent_custom_env(&value)?;
        body.insert("custom_env".into(), value);
    }
    if let Some(value) = resolve_agent_secret_json(
        args.mcp_config.as_deref(),
        args.mcp_config_stdin,
        args.mcp_config_file.as_deref(),
        "mcp-config",
        true,
        environment,
        input,
    )? {
        body.insert("mcp_config".into(), value);
    }
    for (key, value) in [
        ("model", &args.model),
        ("thinking_level", &args.thinking_level),
        ("service_tier", &args.service_tier),
        ("visibility", &args.visibility),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    apply_agent_permission_args(
        args.permission_mode.as_deref(),
        args.public_to_workspace,
        &args.public_to_member,
        &mut body,
    );
    if let Some(value) = args.max_concurrent_tasks {
        body.insert("max_concurrent_tasks".into(), Value::from(value));
    }

    let agent: Value = client
        .post_json("/api/agents", &body)
        .await
        .context("create agent")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agent)?),
        OutputFormat::Table => format!(
            "Agent created: {} ({})\n",
            value_string(&agent, "name"),
            value_string(&agent, "id")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AgentUpdateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    if let Some(value) = args.max_concurrent_tasks {
        if !(1..=50).contains(&value) {
            bail!("--max-concurrent-tasks must be between 1 and 50 (got {value})");
        }
    }
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("name", &args.name),
        ("description", &args.description),
        ("instructions", &args.instructions),
        ("runtime_id", &args.runtime_id),
        ("model", &args.model),
        ("thinking_level", &args.thinking_level),
        ("service_tier", &args.service_tier),
        ("visibility", &args.visibility),
        ("status", &args.status),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(raw) = &args.runtime_config {
        body.insert(
            "runtime_config".into(),
            serde_json::from_str(raw).context("--runtime-config must be valid JSON")?,
        );
    }
    if let Some(raw) = &args.custom_args {
        let values: Vec<String> = serde_json::from_str(raw)
            .map_err(|_| anyhow::anyhow!("--custom-args must be a valid JSON array of strings"))?;
        body.insert("custom_args".into(), serde_json::to_value(values)?);
    }
    if let Some(value) = resolve_agent_secret_json(
        args.mcp_config.as_deref(),
        args.mcp_config_stdin,
        args.mcp_config_file.as_deref(),
        "mcp-config",
        true,
        environment,
        input,
    )? {
        body.insert("mcp_config".into(), value);
    }
    apply_agent_permission_args(
        args.permission_mode.as_deref(),
        args.public_to_workspace,
        &args.public_to_member,
        &mut body,
    );
    if let Some(value) = args.max_concurrent_tasks {
        body.insert("max_concurrent_tasks".into(), Value::from(value));
    }
    if body.is_empty() {
        bail!("no fields to update; use --name, --description, --instructions, --runtime-id, --runtime-config, --model, --thinking-level, --service-tier, --custom-args, --mcp-config, --visibility, --status, or --max-concurrent-tasks (env vars now live behind `cordy agent env set <id>`)");
    }
    let agent: Value = client
        .put_json(&format!("/api/agents/{}", args.id), &body)
        .await
        .context("update agent")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agent)?),
        OutputFormat::Table => format!(
            "Agent updated: {} ({})\n",
            value_string(&agent, "name"),
            value_string(&agent, "id")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_lifecycle(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    action: &str,
    past_tense: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let agent: Value = client
        .post_json(&format!("/api/agents/{id}/{action}"), &Value::Null)
        .await
        .with_context(|| format!("{action} agent"))?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agent)?),
        OutputFormat::Table => format!(
            "Agent {past_tense}: {} ({})\n",
            value_string(&agent, "name"),
            value_string(&agent, "id")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_tasks(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let tasks: Vec<Value> = client
        .get_json(&format!("/api/agents/{id}/tasks"))
        .await
        .context("list agent tasks")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&tasks)?),
        OutputFormat::Table => {
            let mut rows = vec![vec![
                "ID".into(),
                "ISSUE_ID".into(),
                "STATUS".into(),
                "CREATED_AT".into(),
            ]];
            rows.extend(tasks.iter().map(|task| {
                vec![
                    value_string(task, "id"),
                    value_string(task, "issue_id"),
                    value_string(task, "status"),
                    value_string(task, "created_at"),
                ]
            }));
            format_table(&rows)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_avatar(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    file: Option<&Path>,
    output: OutputFormat,
) -> Result<RunOutput> {
    let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(Duration::from_secs(60));
    let client = new_api_client(cli, environment)?.with_request_timeout(timeout);
    let file = file.context("--file is required")?;
    let file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        environment.current_dir().join(file)
    };
    let metadata = fs::metadata(&file).context("file not found")?;
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();
    if !matches!(
        extension.as_str(),
        ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp"
    ) {
        bail!(
            "unsupported file format {:?}: must be .png, .jpg, .jpeg, .gif, or .webp",
            extension
        );
    }
    const MAX_AVATAR_SIZE: u64 = 5 << 20;
    if metadata.len() > MAX_AVATAR_SIZE {
        bail!("file too large: {} bytes (max 5MB)", metadata.len());
    }
    let file_data = fs::read(&file).context("read file")?;
    if file_data.len() as u64 > MAX_AVATAR_SIZE {
        bail!("file too large: {} bytes (max 5MB)", file_data.len());
    }

    let _: Value = client
        .get_json(&format!("/api/agents/{id}"))
        .await
        .context("get agent")?;
    let filename = file.to_string_lossy();
    let upload = client
        .upload_file_with_url(file_data, &filename)
        .await
        .context("upload avatar")?;
    let attachment_id = upload.id;
    let avatar_url = upload.url;
    let _: Value = client
        .put_json(
            &format!("/api/agents/{id}"),
            &serde_json::json!({"avatar_url":&avatar_url}),
        )
        .await
        .context("update agent avatar")?;
    let result = serde_json::json!({
        "id":&attachment_id,
        "agent_id":id,
        "avatar_url":&avatar_url,
    });
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => format_table(&[
            vec!["ID".into(), "AGENT_ID".into(), "AVATAR_URL".into()],
            vec![attachment_id, id.into(), avatar_url],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_skills_list(
    cli: &Cli,
    environment: &Environment,
    agent_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let skills: Vec<Value> = client
        .get_json(&format!("/api/agents/{agent_id}/skills"))
        .await
        .context("list agent skills")?;
    Ok(RunOutput {
        stdout: format_agent_skills(&skills, output, None)?,
        stderr: String::new(),
    })
}

async fn run_agent_skills_mutation(
    cli: &Cli,
    environment: &Environment,
    args: &AgentSkillsMutationArgs,
    additive: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let supplied = args.skill_ids.as_ref().with_context(|| {
        if additive {
            "--skill-ids is required (comma-separated skill IDs)"
        } else {
            "--skill-ids is required (comma-separated skill IDs; use --skill-ids '' to clear all)"
        }
    })?;
    let skill_ids = supplied
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if additive && skill_ids.is_empty() {
        bail!("--skill-ids must include at least one skill ID");
    }
    let path = if additive {
        format!("/api/agents/{}/skills/add", args.agent_id)
    } else {
        format!("/api/agents/{}/skills", args.agent_id)
    };
    let body = serde_json::json!({"skill_ids":skill_ids});
    let skills: Vec<Value> = if additive {
        client
            .post_json(&path, &body)
            .await
            .context("add agent skills")?
    } else {
        client
            .put_json(&path, &body)
            .await
            .context("set agent skills")?
    };
    Ok(RunOutput {
        stdout: format_agent_skills(&skills, args.output, Some(&args.agent_id))?,
        stderr: String::new(),
    })
}

fn format_agent_skills(
    skills: &[Value],
    output: OutputFormat,
    empty_agent_id: Option<&str>,
) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(skills)?));
    }
    if skills.is_empty() {
        if let Some(agent_id) = empty_agent_id {
            return Ok(format!("No skills assigned to agent {agent_id}\n"));
        }
    }
    let mut rows = vec![vec!["ID".into(), "NAME".into(), "DESCRIPTION".into()]];
    rows.extend(skills.iter().map(|skill| {
        vec![
            value_string(skill, "id"),
            value_string(skill, "name"),
            value_string(skill, "description"),
        ]
    }));
    Ok(format_table(&rows))
}

async fn run_agent_env_get(
    cli: &Cli,
    environment: &Environment,
    agent_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let response: Value = client
        .get_json(&format!("/api/agents/{agent_id}/env"))
        .await
        .context("get agent env")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let mut rows = vec![vec!["KEY".into(), "VALUE".into()]];
            if let Some(environment) = response.get("custom_env").and_then(Value::as_object) {
                rows.extend(environment.iter().map(|(key, value)| {
                    vec![
                        key.clone(),
                        value.as_str().map_or_else(|| value.to_string(), Into::into),
                    ]
                }));
            }
            format_table(&rows)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_agent_env_set<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AgentEnvSetArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let custom_env = resolve_agent_secret_json(
        args.custom_env.as_deref(),
        args.custom_env_stdin,
        args.custom_env_file.as_deref(),
        "custom-env",
        false,
        environment,
        input,
    )?
    .context(
        "specify the new env via --custom-env, --custom-env-stdin, or --custom-env-file (pass '{}' to clear)",
    )?;
    validate_agent_custom_env(&custom_env)?;
    let result: Value = client
        .put_json(
            &format!("/api/agents/{}/env", args.agent_id),
            &serde_json::json!({"custom_env":custom_env}),
        )
        .await
        .context("update agent env")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => format!(
            "Env updated for agent {} ({} keys)\n",
            args.agent_id,
            result
                .get("custom_env")
                .and_then(Value::as_object)
                .map_or(0, serde_json::Map::len)
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

#[derive(Clone, Copy)]
enum AgentMcpAction {
    Add,
    Enable,
    Disable,
    Remove,
}

fn agent_mcp_path(agent_id: &str, suffix: &[&str]) -> String {
    let mut url = Url::parse("http://localhost").expect("constant URL");
    {
        let mut segments = url.path_segments_mut().expect("hierarchical URL");
        segments.clear();
        segments.extend(["api", "agents", agent_id.trim(), "mcp-servers"]);
        segments.extend(suffix.iter().copied());
    }
    url.path().into()
}

async fn run_agent_mcp_list(
    cli: &Cli,
    environment: &Environment,
    args: &AgentMcpListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let servers: Vec<WorkspaceMcpServer> = client
        .get_json(&agent_mcp_path(&args.agent_id, &[]))
        .await
        .context("list agent mcp servers")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(&servers, args.output)?,
        stderr: String::new(),
    })
}

async fn run_agent_mcp_mutation(
    cli: &Cli,
    environment: &Environment,
    args: &AgentMcpMutationArgs,
    action: AgentMcpAction,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let agent_id = args.agent_id.trim();
    let server_id = args.server_id.trim();
    let servers: Vec<WorkspaceMcpServer> = match action {
        AgentMcpAction::Add => client
            .post_json(
                &agent_mcp_path(agent_id, &[]),
                &serde_json::json!({"server_id":server_id}),
            )
            .await
            .context("add agent mcp server")?,
        AgentMcpAction::Enable | AgentMcpAction::Disable => client
            .put_json(
                &agent_mcp_path(agent_id, &[server_id, "enabled"]),
                &serde_json::json!({"enabled":matches!(action, AgentMcpAction::Enable)}),
            )
            .await
            .context("update agent mcp server")?,
        AgentMcpAction::Remove => client
            .delete_json(&agent_mcp_path(agent_id, &[server_id]))
            .await
            .context("remove agent mcp server")?,
    };
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(&servers, args.output)?,
        stderr: String::new(),
    })
}

async fn run_agent_copy<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AgentCopyArgs,
    input: &mut R,
) -> Result<RunOutput> {
    if let Some(value) = args.max_concurrent_tasks {
        if !(1..=50).contains(&value) {
            bail!("--max-concurrent-tasks must be between 1 and 50 (got {value})");
        }
    }
    let client = new_api_client(cli, environment)?;
    let source: Value = client
        .get_json(&format!("/api/agents/{}", args.source_agent_id))
        .await
        .context("get source agent")?;
    let source_runtime_id = value_string(&source, "runtime_id");
    let target_runtime_id = match &args.runtime_id {
        Some(value) if value.is_empty() => bail!("--runtime-id must not be empty"),
        Some(value) => value.clone(),
        None if source_runtime_id.is_empty() => {
            bail!("source agent has no runtime; pass --runtime-id to choose a target runtime")
        }
        None => source_runtime_id.clone(),
    };
    let same_runtime = target_runtime_id == source_runtime_id;
    let name = match &args.name {
        Some(value) if value.is_empty() => bail!("--name must not be empty"),
        Some(value) => value.clone(),
        None => format!("{} (copy)", value_string(&source, "name")),
    };
    let mut body = serde_json::Map::from_iter([
        ("name".into(), Value::String(name)),
        ("runtime_id".into(), Value::String(target_runtime_id)),
        (
            "description".into(),
            Value::String(
                args.description
                    .clone()
                    .unwrap_or_else(|| value_string(&source, "description")),
            ),
        ),
        (
            "instructions".into(),
            Value::String(
                args.instructions
                    .clone()
                    .unwrap_or_else(|| value_string(&source, "instructions")),
            ),
        ),
    ]);
    if let Some(avatar) = source.get("avatar_url").filter(|value| !value.is_null()) {
        body.insert("avatar_url".into(), avatar.clone());
    }
    if let Some(raw) = &args.custom_args {
        let custom_args: Vec<String> = serde_json::from_str(raw)
            .map_err(|_| anyhow::anyhow!("--custom-args must be a valid JSON array of strings"))?;
        body.insert("custom_args".into(), serde_json::to_value(custom_args)?);
    } else if let Some(custom_args) = source
        .get("custom_args")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
    {
        body.insert("custom_args".into(), Value::Array(custom_args.clone()));
    }
    if let Some(value) = args.max_concurrent_tasks {
        body.insert("max_concurrent_tasks".into(), Value::from(value));
    } else if let Some(value) =
        copied_agent_max_concurrent_tasks(source.get("max_concurrent_tasks"))
    {
        body.insert("max_concurrent_tasks".into(), Value::from(value));
    }

    if same_runtime {
        for key in ["model", "thinking_level", "service_tier"] {
            let value = value_string(&source, key);
            if !value.is_empty() {
                body.insert(key.into(), Value::String(value));
            }
        }
    } else if args.model.is_none() {
        bail!("copying to a different runtime (--runtime-id) requires --model, because the source model may not exist on the target runtime; pass --model \"\" to accept the target runtime default");
    }
    for (key, value) in [
        ("model", &args.model),
        ("thinking_level", &args.thinking_level),
        ("service_tier", &args.service_tier),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }

    let permission_override = args.permission_mode.is_some()
        || args.public_to_workspace.is_some()
        || !args.public_to_member.is_empty()
        || args.visibility.is_some();
    if permission_override {
        if let Some(visibility) = &args.visibility {
            body.insert("visibility".into(), Value::String(visibility.clone()));
        }
        apply_agent_permission_args(
            args.permission_mode.as_deref(),
            args.public_to_workspace,
            &args.public_to_member,
            &mut body,
        );
    } else {
        let permission_mode = value_string(&source, "permission_mode");
        if !permission_mode.is_empty() {
            body.insert("permission_mode".into(), Value::String(permission_mode));
        }
        if let Some(targets) = source
            .get("invocation_targets")
            .filter(|value| !value.is_null())
        {
            body.insert("invocation_targets".into(), targets.clone());
        }
    }
    if !args.no_skills {
        let skill_ids = source
            .get("skills")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|skill| skill.get("id").and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        if !skill_ids.is_empty() {
            body.insert("skill_ids".into(), serde_json::to_value(skill_ids)?);
        }
    }
    if let Some(custom_env) = resolve_agent_secret_json(
        args.custom_env.as_deref(),
        args.custom_env_stdin,
        args.custom_env_file.as_deref(),
        "custom-env",
        false,
        environment,
        input,
    )? {
        validate_agent_custom_env(&custom_env)?;
        body.insert("custom_env".into(), custom_env);
    }
    if let Some(mcp_config) = resolve_agent_secret_json(
        args.mcp_config.as_deref(),
        args.mcp_config_stdin,
        args.mcp_config_file.as_deref(),
        "mcp-config",
        true,
        environment,
        input,
    )? {
        body.insert("mcp_config".into(), mcp_config);
    }
    if let Some(runtime_config) = &args.runtime_config {
        body.insert(
            "runtime_config".into(),
            serde_json::from_str(runtime_config).context("--runtime-config must be valid JSON")?,
        );
    }

    let agent: Value = client
        .post_json("/api/agents", &body)
        .await
        .context("copy agent")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agent)?),
            OutputFormat::Table => format!(
                "Agent copied: {} ({})\n",
                value_string(&agent, "name"),
                value_string(&agent, "id")
            ),
        },
        stderr: String::new(),
    })
}

fn copied_agent_max_concurrent_tasks(value: Option<&Value>) -> Option<i32> {
    let value = value?.as_f64()?;
    if value.fract() != 0.0 || !(1.0..=50.0).contains(&value) {
        return None;
    }
    Some(value as i32)
}

fn apply_agent_permission_args(
    permission_mode: Option<&str>,
    public_to_workspace: Option<bool>,
    public_to_member: &[String],
    body: &mut serde_json::Map<String, Value>,
) {
    if permission_mode.is_none() && public_to_workspace.is_none() && public_to_member.is_empty() {
        return;
    }
    body.insert(
        "permission_mode".into(),
        Value::String(
            permission_mode
                .map(str::to_owned)
                .unwrap_or_else(|| "public_to".into()),
        ),
    );
    let mut targets = Vec::new();
    if public_to_workspace == Some(true) {
        targets.push(serde_json::json!({"target_type":"workspace"}));
    }
    targets.extend(
        public_to_member
            .iter()
            .map(|member| serde_json::json!({"target_type":"member","target_id":member})),
    );
    body.insert("invocation_targets".into(), Value::Array(targets));
}

fn validate_agent_custom_env(value: &Value) -> Result<()> {
    let Some(object) = value.as_object() else {
        bail!("--custom-env must be a valid JSON object of string keys and string values");
    };
    if object.values().any(|value| !value.is_string()) {
        bail!("--custom-env must be a valid JSON object of string keys and string values");
    }
    Ok(())
}

fn resolve_agent_secret_json<R: Read>(
    inline: Option<&str>,
    from_stdin: bool,
    file: Option<&Path>,
    flag: &str,
    allow_null: bool,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<Value>> {
    let count =
        usize::from(inline.is_some()) + usize::from(from_stdin) + usize::from(file.is_some());
    if count == 0 {
        return Ok(None);
    }
    if count > 1 {
        bail!("--{flag}, --{flag}-stdin, and --{flag}-file are mutually exclusive; pick one");
    }
    let raw = if let Some(raw) = inline {
        raw.to_string()
    } else if from_stdin {
        let mut raw = String::new();
        input
            .read_to_string(&mut raw)
            .with_context(|| format!("read --{flag}-stdin"))?;
        if raw.trim().is_empty() {
            if allow_null {
                bail!("--{flag}-stdin: empty input; pass 'null' to clear");
            }
            bail!("--{flag}-stdin: empty input; pass '{{}}' to clear");
        }
        raw
    } else {
        let path = file.context("secret file path")?;
        if path.as_os_str().is_empty() {
            bail!("--{flag}-file: path must not be empty");
        }
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let raw = fs::read_to_string(&path).with_context(|| format!("read --{flag}-file"))?;
        if raw.trim().is_empty() {
            if allow_null {
                bail!(
                    "--{flag}-file {:?}: empty contents; pass 'null' to clear",
                    path
                );
            }
            bail!(
                "--{flag}-file {:?}: empty contents; pass '{{}}' to clear",
                path
            );
        }
        raw
    };
    if raw.trim().is_empty() {
        if allow_null {
            bail!("--{flag}: empty input; pass 'null' to clear or a JSON object to set");
        }
        bail!("--{flag}: empty input; pass '{{}}' to clear");
    }
    let value: Value = serde_json::from_str(raw.trim()).map_err(|_| {
        if allow_null {
            anyhow::anyhow!("--{flag} must be a valid JSON object, or 'null' to clear")
        } else {
            anyhow::anyhow!("--{flag} must be a valid JSON object of string keys and string values")
        }
    })?;
    if value.is_null() && allow_null {
        return Ok(Some(value));
    }
    if value.is_null() {
        return Ok(Some(Value::Object(serde_json::Map::new())));
    }
    if !value.is_object() {
        if allow_null {
            bail!("--{flag} must be a valid JSON object, or 'null' to clear");
        }
        bail!("--{flag} must be a valid JSON object of string keys and string values");
    }
    Ok(Some(value))
}

fn format_agent_list_table(agents: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "STATUS".into(),
        "RUNTIME".into(),
        "ARCHIVED".into(),
    ]];
    rows.extend(agents.iter().map(|agent| {
        vec![
            value_string(agent, "id"),
            value_string(agent, "name"),
            value_string(agent, "status"),
            value_string(agent, "runtime_mode"),
            if value_string(agent, "archived_at").is_empty() {
                String::new()
            } else {
                "yes".into()
            },
        ]
    }));
    format_table(&rows)
}

fn format_agent_details_table(agent: &Value) -> String {
    format_table(&[
        vec![
            "ID".into(),
            "NAME".into(),
            "STATUS".into(),
            "RUNTIME".into(),
            "VISIBILITY".into(),
            "AVATAR_URL".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            value_string(agent, "id"),
            value_string(agent, "name"),
            value_string(agent, "status"),
            value_string(agent, "runtime_mode"),
            value_string(agent, "visibility"),
            value_string(agent, "avatar_url"),
            value_string(agent, "description"),
        ],
    ])
}

fn format_chat_read(response: &Value, output: OutputFormat, overview: bool) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(response)?));
    }
    if let Some(note) = response
        .get("note")
        .and_then(Value::as_str)
        .filter(|note| !note.is_empty())
    {
        return Ok(format!("{note}\n"));
    }
    let messages = response
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows = vec![if overview {
        vec![
            "TS".into(),
            "ROLE".into(),
            "AUTHOR".into(),
            "THREAD_ID".into(),
            "REPLIES".into(),
            "TEXT".into(),
        ]
    } else {
        vec!["TS".into(), "ROLE".into(), "AUTHOR".into(), "TEXT".into()]
    }];
    rows.extend(messages.iter().map(|message| {
        let mut row = vec![
            value_string(message, "ts"),
            value_string(message, "role"),
            value_string(message, "author"),
        ];
        if overview {
            row.push(value_string(message, "thread_id"));
            row.push(chat_reply_count(message));
        }
        row.push(value_string(message, "text"));
        row
    }));
    Ok(format_table(&rows))
}

async fn run_chat_read(
    cli: &Cli,
    environment: &Environment,
    base_path: &str,
    thread_id: Option<&str>,
    args: &ChatReadArgs,
    overview: bool,
) -> Result<RunOutput> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if let Some(before) = args.before.as_deref().filter(|before| !before.is_empty()) {
        serializer.append_pair("before", before);
    }
    if let Some(thread_id) = thread_id.filter(|thread_id| !thread_id.is_empty()) {
        serializer.append_pair("id", thread_id);
    }
    if args.limit > 0 {
        serializer.append_pair("limit", &args.limit.to_string());
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        base_path.into()
    } else {
        format!("{base_path}?{query}")
    };
    let client = new_api_client(cli, environment)?;
    let response: Value = client.get_json(&path).await.context("read chat")?;
    Ok(RunOutput {
        stdout: format_chat_read(&response, args.output, overview)?,
        stderr: String::new(),
    })
}

fn escape_markdown_label(label: &str) -> String {
    let mut escaped = String::with_capacity(label.len());
    for character in label.chars() {
        if matches!(character, '\\' | '[' | ']' | '(' | ')') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

async fn run_attachment_upload(
    cli: &Cli,
    environment: &Environment,
    path: &Path,
    task: Option<&str>,
) -> Result<RunOutput> {
    let task_id = task
        .filter(|task| !task.is_empty())
        .or_else(|| environment.raw("CORDY_TASK_ID"))
        .unwrap_or_default();
    if task_id.is_empty() {
        bail!(
            "no chat task in context: run inside a chat task (CORDY_TASK_ID set) or pass --task <id>"
        );
    }
    let path_text = path.to_string_lossy();
    if path_text.starts_with("http://") || path_text.starts_with("https://") {
        bail!("upload accepts a local file path, not a URL: {path_text}");
    }
    let read_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        environment.current_dir().join(path)
    };
    // Chat-task attachment uploads are performed with the task's machine
    // credentials.  Keep the source file inside the task workdir so a task
    // cannot exfiltrate arbitrary daemon-readable files via an absolute or
    // parent-traversal path.  Human-facing attachment flows have their own
    // explicit external-file policy; this task-scoped command has no such
    // override.
    ensure_file_within_workdir(path, environment.current_dir(), false, "attachment")?;
    let data =
        fs::read(&read_path).with_context(|| format!("read file {}", path.to_string_lossy()))?;
    let request_timeout =
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(std::time::Duration::from_secs(60));
    let client = new_api_client(cli, environment)?.with_request_timeout(request_timeout);
    let attachment = client
        .upload_chat_attachment(data, &path_text, task_id)
        .await
        .context("upload attachment")?;
    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .unwrap_or(&path_text);
    let label = escape_markdown_label(filename);
    let markdown = if attachment.content_type.starts_with("image/") {
        format!("![{label}]({})", attachment.markdown_url)
    } else {
        format!("!file[{label}]({})", attachment.markdown_url)
    };
    Ok(RunOutput {
        stdout: format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "id":attachment.id,
                "filename":filename,
                "markdown_url":attachment.markdown_url,
                "markdown":markdown
            }))?
        ),
        stderr: format!("Uploaded: {filename}\n"),
    })
}

async fn run_attachment_download(
    cli: &Cli,
    environment: &Environment,
    attachment_id: &str,
    output_dir: &Path,
) -> Result<RunOutput> {
    let request_timeout =
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(std::time::Duration::from_secs(60));
    let client = new_api_client(cli, environment)?.with_request_timeout(request_timeout);
    let attachment: Value = client
        .get_json(&format!("/api/attachments/{attachment_id}"))
        .await
        .context("get attachment")?;
    let download_url = value_string(&attachment, "download_url");
    if download_url.is_empty() {
        bail!("attachment has no download URL");
    }
    let raw_filename = value_string(&attachment, "filename");
    let filename = Path::new(&raw_filename)
        .file_name()
        .and_then(|filename| filename.to_str())
        .filter(|filename| !filename.is_empty() && *filename != ".")
        .unwrap_or(attachment_id);
    let data = client
        .download_file(&download_url)
        .await
        .context("download file")?;
    let directory = if output_dir.is_absolute() {
        output_dir.to_path_buf()
    } else {
        environment.current_dir().join(output_dir)
    };
    if !output_dir.as_os_str().is_empty() {
        fs::create_dir_all(&directory).context("create output directory")?;
    }
    let destination = directory.join(filename);
    fs::write(&destination, data).context("write file")?;
    let absolute = fs::canonicalize(&destination).unwrap_or(destination);
    let path = absolute.to_string_lossy();
    Ok(RunOutput {
        stdout: format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "id":value_string(&attachment, "id"),
                "filename":filename,
                "path":path,
                "size":value_string(&attachment, "size_bytes")
            }))?
        ),
        stderr: format!("Downloaded: {path}\n"),
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkspaceRepo {
    url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
}

#[derive(Debug, Deserialize)]
struct RepoWorkspace {
    id: String,
    #[serde(default)]
    repos: Vec<WorkspaceRepo>,
}

#[derive(Debug, Serialize)]
struct RepoMutationResult {
    workspace_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    added: Vec<WorkspaceRepo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    updated: Vec<WorkspaceRepo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<WorkspaceRepo>,
    repos: Vec<WorkspaceRepo>,
}

fn repo_urls(flag_urls: &[String], positional: &[String]) -> Result<Vec<String>> {
    let mut raw = Vec::with_capacity(flag_urls.len() + positional.len());
    raw.extend(flag_urls.iter());
    raw.extend(positional.iter());
    if raw.is_empty() {
        bail!("at least one repository URL is required");
    }
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for url in raw {
        let url = url.trim();
        if url.is_empty() {
            bail!("repository URL cannot be empty");
        }
        if seen.insert(url.to_string()) {
            urls.push(url.to_string());
        }
    }
    Ok(urls)
}

fn required_workspace_id(cli: &Cli, environment: &Environment) -> Result<String> {
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
    Ok(workspace_id)
}

async fn fetch_repo_workspace(client: &ApiClient, workspace_id: &str) -> Result<RepoWorkspace> {
    client
        .get_json(&format!("/api/workspaces/{workspace_id}"))
        .await
        .context("get workspace")
}

async fn patch_workspace_repos(
    client: &ApiClient,
    workspace_id: &str,
    repos: &[WorkspaceRepo],
) -> Result<RepoWorkspace> {
    client
        .patch_json(
            &format!("/api/workspaces/{workspace_id}"),
            &serde_json::json!({"repos":repos}),
        )
        .await
        .context("update workspace repos")
}

fn format_repo_list(repos: &[WorkspaceRepo]) -> String {
    let mut rows = vec![vec!["URL".into(), "DESCRIPTION".into()]];
    rows.extend(
        repos
            .iter()
            .map(|repo| vec![repo.url.clone(), repo.description.clone()]),
    );
    format_table(&rows)
}

async fn run_repo_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let workspace_id = required_workspace_id(cli, environment)?;
    let client = new_api_client(cli, environment)?;
    let workspace = fetch_repo_workspace(&client, &workspace_id).await?;
    Ok(match output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&workspace.repos)?),
            stderr: String::new(),
        },
        OutputFormat::Table if workspace.repos.is_empty() => RunOutput {
            stdout: String::new(),
            stderr: "No repositories found.\n".into(),
        },
        OutputFormat::Table => RunOutput {
            stdout: format_repo_list(&workspace.repos),
            stderr: String::new(),
        },
    })
}

async fn run_repo_add(
    cli: &Cli,
    environment: &Environment,
    args: &RepoMutationArgs,
) -> Result<RunOutput> {
    let urls = repo_urls(&args.flag_urls, &args.urls)?;
    if args.description.is_some() && urls.len() > 1 {
        bail!("--description can only be used when adding one repository URL");
    }
    let workspace_id = required_workspace_id(cli, environment)?;
    let client = new_api_client(cli, environment)?;
    let mut workspace = fetch_repo_workspace(&client, &workspace_id).await?;
    let mut index_by_url = workspace
        .repos
        .iter()
        .enumerate()
        .map(|(index, repo)| (repo.url.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut added = Vec::new();
    let mut updated = Vec::new();
    for url in urls {
        if let Some(index) = index_by_url.get(&url).copied() {
            if let Some(description) = &args.description {
                if workspace.repos[index].description != *description {
                    workspace.repos[index].description = description.clone();
                    updated.push(workspace.repos[index].clone());
                }
            }
            continue;
        }
        let repo = WorkspaceRepo {
            url: url.clone(),
            description: args.description.clone().unwrap_or_default(),
        };
        index_by_url.insert(url, workspace.repos.len());
        workspace.repos.push(repo.clone());
        added.push(repo);
    }
    if !added.is_empty() || !updated.is_empty() {
        workspace = patch_workspace_repos(&client, &workspace_id, &workspace.repos).await?;
    }
    let result = RepoMutationResult {
        workspace_id: workspace.id,
        added,
        updated,
        removed: Vec::new(),
        repos: workspace.repos,
    };
    let stdout =
        match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table if result.added.is_empty() && result.updated.is_empty() => {
                "No repository changes.\n".into()
            }
            OutputFormat::Table => {
                let mut rows = vec![vec!["ACTION".into(), "URL".into(), "DESCRIPTION".into()]];
                rows.extend(
                    result.added.iter().map(|repo| {
                        vec!["added".into(), repo.url.clone(), repo.description.clone()]
                    }),
                );
                rows.extend(result.updated.iter().map(|repo| {
                    vec!["updated".into(), repo.url.clone(), repo.description.clone()]
                }));
                format_table(&rows)
            }
        };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn run_repo_remove(
    cli: &Cli,
    environment: &Environment,
    args: &RepoRemoveArgs,
) -> Result<RunOutput> {
    let urls = repo_urls(&args.flag_urls, &args.urls)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let client = new_api_client(cli, environment)?;
    let workspace = fetch_repo_workspace(&client, &workspace_id).await?;
    let remove_set = urls.iter().cloned().collect::<HashSet<_>>();
    let (removed, repos): (Vec<_>, Vec<_>) = workspace
        .repos
        .into_iter()
        .partition(|repo| remove_set.contains(&repo.url));
    let removed_set = removed
        .iter()
        .map(|repo| repo.url.as_str())
        .collect::<HashSet<_>>();
    let missing = urls
        .iter()
        .filter(|url| !removed_set.contains(url.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "repository not found in workspace registry: {}",
            missing.join(", ")
        );
    }
    let workspace = patch_workspace_repos(&client, &workspace_id, &repos).await?;
    let result = RepoMutationResult {
        workspace_id: workspace.id,
        added: Vec::new(),
        updated: Vec::new(),
        removed,
        repos: workspace.repos,
    };
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => {
                let mut rows = vec![vec!["REMOVED URL".into(), "DESCRIPTION".into()]];
                rows.extend(
                    result
                        .removed
                        .iter()
                        .map(|repo| vec![repo.url.clone(), repo.description.clone()]),
                );
                format_table(&rows)
            }
        },
        stderr: String::new(),
    })
}

fn repo_checkout_retry_delay(
    value: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> std::time::Duration {
    const DEFAULT_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
    const MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
    let value = value.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        if seconds >= 0 {
            return std::time::Duration::from_secs(seconds as u64).min(MAX_DELAY);
        }
    }
    if let Ok(retry_at) = chrono::DateTime::parse_from_rfc2822(value) {
        let delay = retry_at.with_timezone(&chrono::Utc) - now;
        return delay.to_std().unwrap_or_default().min(MAX_DELAY);
    }
    DEFAULT_DELAY
}

async fn run_repo_checkout(
    environment: &Environment,
    repo_url: &str,
    checkout_ref: Option<&str>,
) -> Result<RunOutput> {
    let daemon_port = environment.raw("CORDY_DAEMON_PORT").unwrap_or_default();
    if daemon_port.is_empty() {
        bail!(
            "CORDY_DAEMON_PORT not set (this command is intended to be run by an agent inside a daemon task)"
        );
    }
    let token = environment.raw("CORDY_TOKEN").unwrap_or_default();
    if token.is_empty() {
        bail!("CORDY_TOKEN not set (repo checkout requires the active task credential)");
    }
    let body = serde_json::json!({
        "url":repo_url,
        "workspace_id":environment.raw("CORDY_WORKSPACE_ID").unwrap_or_default(),
        "workdir":environment.current_dir(),
        "ref":checkout_ref.unwrap_or_default(),
        "agent_name":environment.raw("CORDY_AGENT_NAME").unwrap_or_default(),
        "task_id":environment.raw("CORDY_TASK_ID").unwrap_or_default(),
        "checkout_mode":environment.raw("CORDY_REPO_CHECKOUT_MODE").unwrap_or_default().trim(),
        "retry_busy":true
    });
    let checkout_url = format!("http://127.0.0.1:{daemon_port}/repo/checkout");
    let client = reqwest::Client::new();
    let checkout = async {
        loop {
            let response = client
                .post(&checkout_url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .context("connect to daemon")?;
            let status = response.status();
            let retryable = response
                .headers()
                .get("X-Cordy-Retryable")
                .and_then(|value| value.to_str().ok())
                == Some("repo-busy");
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let response_body = response
                .text()
                .await
                .context("read daemon checkout response")?;
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE && retryable {
                tokio::time::sleep(repo_checkout_retry_delay(&retry_after, chrono::Utc::now()))
                    .await;
                continue;
            }
            if status != reqwest::StatusCode::OK {
                bail!("checkout failed: {response_body}");
            }
            let result: Value = serde_json::from_str(&response_body).context("parse response")?;
            let path = value_string(&result, "path");
            let branch = value_string(&result, "branch_name");
            return Ok(RunOutput {
                stdout: format!("{path}\n"),
                stderr: format!("Checked out {repo_url} → {path} (branch: {branch})\n"),
            });
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5 * 60), checkout)
        .await
        .map_err(|_| anyhow::anyhow!("connect to daemon: deadline exceeded"))?
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
    resolve_project_reference(client, workspace_id, raw)
        .await
        .map(|(id, _)| id)
}

async fn resolve_project_reference(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<(String, String)> {
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok((input.into(), input.into()));
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
        [project] => {
            let id = value_string(project, "id");
            let title = value_string(project, "title");
            Ok((id.clone(), if title.is_empty() { id } else { title }))
        }
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
        .map_err(|error| anyhow::anyhow!("resolve task run: {error}"))?;
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
        .map_err(|error| anyhow::anyhow!("resolve task run: {error}"))?;
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

fn project_lead(project: &Value, actors: &IssueActorNames) -> String {
    let actor_type = value_string(project, "lead_type");
    let actor_id = value_string(project, "lead_id");
    if actor_type.is_empty() || actor_id.is_empty() {
        return String::new();
    }
    let key = format!("{actor_type}:{actor_id}");
    actors
        .0
        .get(&key)
        .map_or(key, |name| format!("{actor_type}:{name}"))
}

fn project_actor_inputs(projects: &[Value]) -> Vec<Value> {
    projects
        .iter()
        .map(|project| {
            serde_json::json!({
                "assignee_type":project.get("lead_type").cloned().unwrap_or(Value::Null),
                "assignee_id":project.get("lead_id").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn format_project_list_table(
    projects: &[Value],
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TITLE".into(),
        "STATUS".into(),
        "LEAD".into(),
        "CREATED".into(),
    ]];
    rows.extend(projects.iter().map(|project| {
        vec![
            display_id(&value_string(project, "id"), full_id),
            value_string(project, "title"),
            value_string(project, "status"),
            project_lead(project, actors),
            value_string(project, "created_at")
                .chars()
                .take(10)
                .collect(),
        ]
    }));
    format_table(&rows)
}

async fn run_project_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    full_id: bool,
    status: Option<&str>,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if !workspace_id.is_empty() {
        serializer.append_pair("workspace_id", &workspace_id);
    }
    if let Some(status) = status.filter(|status| !status.is_empty()) {
        serializer.append_pair("status", status);
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        "/api/projects".into()
    } else {
        format!("/api/projects?{query}")
    };
    let result: Value = client.get_json(&path).await.context("list projects")?;
    let projects = result
        .get("projects")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(projects)?),
        OutputFormat::Table => {
            let inputs = project_actor_inputs(projects);
            let actors = load_issue_actor_names(&client, &workspace_id, &inputs).await;
            format_project_list_table(projects, &actors, full_id)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn format_project_details_table(project: &Value, actors: &IssueActorNames) -> String {
    format_table(&[
        vec![
            "ID".into(),
            "TITLE".into(),
            "STATUS".into(),
            "LEAD".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            value_string(project, "id"),
            value_string(project, "title"),
            value_string(project, "status"),
            project_lead(project, actors),
            value_string(project, "description"),
        ],
    ])
}

async fn run_project_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let project: Value = client
        .get_json(&format!("/api/projects/{project_id}"))
        .await
        .context("get project")?;
    let resource_count = project
        .get("resource_count")
        .and_then(Value::as_f64)
        .unwrap_or_default() as i64;
    let stderr = if resource_count > 0 {
        format!(
            "{resource_count} resource(s) attached — run `cordy project resource list {project_id}` to view.\n"
        )
    } else {
        String::new()
    };
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&project)?),
        OutputFormat::Table => {
            let inputs = project_actor_inputs(std::slice::from_ref(&project));
            let actors = load_issue_actor_names(&client, &workspace_id, &inputs).await;
            format_project_details_table(&project, &actors)
        }
    };
    Ok(RunOutput { stdout, stderr })
}

const PROJECT_STATUSES: &[&str] = &["planned", "in_progress", "paused", "completed", "cancelled"];

fn validate_project_status(status: &str) -> Result<()> {
    if PROJECT_STATUSES.contains(&status) {
        Ok(())
    } else {
        bail!(
            "invalid status {status:?}; valid values: {}",
            PROJECT_STATUSES.join(", ")
        )
    }
}

fn format_project_mutation(project: &Value, output: OutputFormat) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(project)?)),
        OutputFormat::Table => Ok(format_table(&[
            vec!["ID".into(), "TITLE".into(), "STATUS".into()],
            vec![
                value_string(project, "id"),
                value_string(project, "title"),
                value_string(project, "status"),
            ],
        ])),
    }
}

async fn resolve_project_lead(
    client: &ApiClient,
    workspace_id: &str,
    lead: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_subscriber_name(client, workspace_id, lead)
        .await
        .map_err(|error| anyhow::anyhow!("resolve lead: {error}"))
}

async fn run_project_create(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectCreateArgs,
) -> Result<RunOutput> {
    let title = args
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .context("--title is required")?;
    if let Some(status) = args.status.as_deref().filter(|status| !status.is_empty()) {
        validate_project_status(status)?;
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let mut body = serde_json::Map::from_iter([("title".into(), Value::String(title.into()))]);
    for (key, value) in [
        ("description", args.description.as_deref()),
        ("status", args.status.as_deref()),
        ("icon", args.icon.as_deref()),
        ("start_date", args.start_date.as_deref()),
        ("due_date", args.due_date.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            body.insert(key.into(), Value::String(value.into()));
        }
    }
    if let Some(lead) = args.lead.as_deref().filter(|lead| !lead.is_empty()) {
        let lead = resolve_project_lead(&client, &workspace_id, lead).await?;
        body.insert("lead_type".into(), Value::String(lead.actor_type));
        body.insert("lead_id".into(), Value::String(lead.id));
    }
    let resources = args
        .repo
        .iter()
        .map(|repo| repo.trim())
        .filter(|repo| !repo.is_empty())
        .map(|repo| {
            serde_json::json!({
                "resource_type":"github_repo",
                "resource_ref":{"url":repo}
            })
        })
        .collect::<Vec<_>>();
    if !resources.is_empty() {
        body.insert("resources".into(), Value::Array(resources));
    }
    let project: Value = client
        .post_json("/api/projects", &body)
        .await
        .context("create project")?;
    Ok(RunOutput {
        stdout: format_project_mutation(&project, args.output)?,
        stderr: String::new(),
    })
}

async fn run_project_update(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectUpdateArgs,
) -> Result<RunOutput> {
    if let Some(status) = &args.status {
        validate_project_status(status)?;
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, &args.id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("title", args.title.as_ref()),
        ("description", args.description.as_ref()),
        ("status", args.status.as_ref()),
        ("icon", args.icon.as_ref()),
        ("start_date", args.start_date.as_ref()),
        ("due_date", args.due_date.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(lead) = &args.lead {
        let lead = resolve_project_lead(&client, &workspace_id, lead).await?;
        body.insert("lead_type".into(), Value::String(lead.actor_type));
        body.insert("lead_id".into(), Value::String(lead.id));
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --status, --description, --icon, --lead, --start-date, --due-date"
        );
    }
    let project: Value = client
        .put_json(&format!("/api/projects/{project_id}"), &body)
        .await
        .context("update project")?;
    Ok(RunOutput {
        stdout: format_project_mutation(&project, args.output)?,
        stderr: String::new(),
    })
}

async fn run_project_delete(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    _output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (project_id, display) = resolve_project_reference(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    client
        .delete(&format!("/api/projects/{project_id}"))
        .await
        .context("delete project")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Project {display} deleted.\n"),
    })
}

async fn run_project_status(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    status: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    validate_project_status(status)?;
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let project: Value = client
        .put_json(
            &format!("/api/projects/{project_id}"),
            &serde_json::json!({"status":status}),
        )
        .await
        .context("update status")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&project)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!(
            "Project {} status changed to {status}.\n",
            value_string(&project, "title")
        ),
    })
}

fn summarize_project_resource_ref(resource_ref: &Value) -> String {
    let Some(object) = resource_ref.as_object() else {
        return String::new();
    };
    let url = value_string(resource_ref, "url");
    if !url.is_empty() {
        let checkout_ref = value_string(resource_ref, "ref");
        return if checkout_ref.trim().is_empty() {
            url
        } else {
            format!("{url} @ {}", checkout_ref.trim())
        };
    }
    let local_path = value_string(resource_ref, "local_path");
    if !local_path.is_empty() {
        return local_path;
    }
    serde_json::to_string(object).unwrap_or_default()
}

fn project_resources(result: &Value) -> &[Value] {
    result
        .get("resources")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn format_project_resources(resources: &[Value], full_id: bool) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TYPE".into(),
        "REF".into(),
        "LABEL".into(),
    ]];
    rows.extend(resources.iter().map(|resource| {
        vec![
            display_id(&value_string(resource, "id"), full_id),
            value_string(resource, "resource_type"),
            summarize_project_resource_ref(resource.get("resource_ref").unwrap_or(&Value::Null)),
            value_string(resource, "label"),
        ]
    }));
    format_table(&rows)
}

async fn run_project_resource_list(
    cli: &Cli,
    environment: &Environment,
    project: &str,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, project)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let result: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("list project resources")?;
    let resources = project_resources(&result);
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(resources)?),
            OutputFormat::Table => format_project_resources(resources, full_id),
        },
        stderr: String::new(),
    })
}

fn parse_generic_resource_ref(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|error| anyhow::anyhow!("--ref is not valid JSON: {error}"))
}

fn build_project_resource_add_ref(args: &ProjectResourceAddArgs) -> Result<Value> {
    let resource_type = args.resource_type.trim();
    if resource_type.is_empty() {
        bail!("--type is required");
    }
    if let Some(raw) = &args.resource_ref {
        let raw = raw.trim();
        if !raw.is_empty()
            && (resource_type != "github_repo" || raw.starts_with('{') || raw.starts_with('['))
        {
            return parse_generic_resource_ref(raw);
        }
        if resource_type != "github_repo" {
            bail!("--ref must be a JSON resource_ref payload for resource type {resource_type:?}");
        }
    }
    match resource_type {
        "github_repo" => {
            let url = args
                .url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .context("github_repo requires --url (or pass a JSON payload via --ref)")?;
            let mut resource_ref = serde_json::Map::from_iter([(
                "url".into(),
                Value::String(url.into()),
            )]);
            if let Some(hint) = args
                .default_branch_hint
                .as_deref()
                .map(str::trim)
                .filter(|hint| !hint.is_empty())
            {
                resource_ref.insert("default_branch_hint".into(), Value::String(hint.into()));
            }
            if let Some(checkout_ref) = args
                .resource_ref
                .as_deref()
                .map(str::trim)
                .filter(|checkout_ref| !checkout_ref.is_empty())
            {
                resource_ref.insert("ref".into(), Value::String(checkout_ref.into()));
            }
            Ok(Value::Object(resource_ref))
        }
        "local_directory" => {
            let local_path = args
                .local_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty());
            let daemon_id = args
                .daemon_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty());
            let (Some(local_path), Some(daemon_id)) = (local_path, daemon_id) else {
                bail!("local_directory requires --local-path and --daemon-id (or pass a JSON payload via --ref)");
            };
            let mut resource_ref = serde_json::Map::from_iter([
                ("local_path".into(), Value::String(local_path.into())),
                ("daemon_id".into(), Value::String(daemon_id.into())),
            ]);
            for (key, value) in [
                ("label", args.ref_label.as_deref()),
                ("execution_mode", args.execution_mode.as_deref()),
            ] {
                if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                    resource_ref.insert(key.into(), Value::String(value.into()));
                }
            }
            Ok(Value::Object(resource_ref))
        }
        _ => bail!(
            "type {resource_type:?} has no built-in CLI shortcut; pass the payload via --ref '<json>'"
        ),
    }
}

async fn run_project_resource_add(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectResourceAddArgs,
) -> Result<RunOutput> {
    let resource_type = args.resource_type.trim();
    let resource_ref = build_project_resource_add_ref(args)?;
    let mut body = serde_json::Map::from_iter([
        ("resource_type".into(), Value::String(resource_type.into())),
        ("resource_ref".into(), resource_ref),
    ]);
    if let Some(label) = args.label.as_deref().filter(|label| !label.is_empty()) {
        body.insert("label".into(), Value::String(label.into()));
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, &args.project_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let resource: Value = client
        .post_json(&format!("/api/projects/{project_id}/resources"), &body)
        .await
        .context("add project resource")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&resource)?),
        OutputFormat::Table => format_table(&[
            vec!["ID".into(), "TYPE".into(), "REF".into()],
            vec![
                value_string(&resource, "id"),
                value_string(&resource, "resource_type"),
                summarize_project_resource_ref(
                    resource.get("resource_ref").unwrap_or(&Value::Null),
                ),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn resolve_project_resource_reference(
    client: &ApiClient,
    project_id: &str,
    raw: &str,
) -> Result<(String, String)> {
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok((input.into(), input.into()));
    }
    let Some(prefix) = normalize_uuid_prefix(input) else {
        if input.is_empty() {
            bail!("resolve project resource: project resource id is required");
        }
        let compact = input.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve project resource: expected a full UUID or at least 4 hex characters, got {raw:?}"
            );
        }
        bail!(
            "resolve project resource: expected a UUID prefix containing only hex characters, got {raw:?}"
        );
    };
    let result: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("resolve project resource")?;
    let mut matches = project_resources(&result)
        .iter()
        .filter_map(|resource| {
            let id = value_string(resource, "id");
            if id.is_empty() || !compact_uuid(&id).starts_with(&prefix) {
                return None;
            }
            let label = value_string(resource, "label");
            let resource_type = value_string(resource, "resource_type");
            Some((
                id.clone(),
                if label.is_empty() {
                    if resource_type.is_empty() {
                        id
                    } else {
                        resource_type
                    }
                } else {
                    label
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [(id, display)] => Ok((id.clone(), display.clone())),
        [] => bail!(
            "no project resource found matching id prefix {raw:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous project resource id prefix {raw:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

fn seed_resource_ref(
    existing: Option<&serde_json::Map<String, Value>>,
    keys: &[&str],
) -> serde_json::Map<String, Value> {
    let mut resource_ref = serde_json::Map::new();
    if let Some(existing) = existing {
        for key in keys {
            if let Some(value) = existing
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                resource_ref.insert((*key).into(), Value::String(value.into()));
            }
        }
    }
    resource_ref
}

fn build_project_resource_update_ref(
    args: &ProjectResourceUpdateArgs,
    resource_type: &str,
    existing: Option<&serde_json::Map<String, Value>>,
) -> Result<Option<Value>> {
    if let Some(raw) = &args.resource_ref {
        let raw = raw.trim();
        if !raw.is_empty()
            && (resource_type != "github_repo" || raw.starts_with('{') || raw.starts_with('['))
        {
            return parse_generic_resource_ref(raw).map(Some);
        }
        if resource_type != "github_repo" {
            bail!("--ref must be a JSON resource_ref payload for resource type {resource_type:?}");
        }
    }
    match resource_type {
        "github_repo" => {
            if args.url.is_none()
                && args.default_branch_hint.is_none()
                && args.resource_ref.is_none()
            {
                return Ok(None);
            }
            let mut resource_ref =
                seed_resource_ref(existing, &["url", "default_branch_hint", "ref"]);
            if let Some(url) = &args.url {
                let url = url.trim();
                if url.is_empty() {
                    bail!("--url cannot be empty");
                }
                resource_ref.insert("url".into(), Value::String(url.into()));
            }
            for (key, value) in [
                ("default_branch_hint", args.default_branch_hint.as_deref()),
                ("ref", args.resource_ref.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        resource_ref.remove(key);
                    } else {
                        resource_ref.insert(key.into(), Value::String(value.into()));
                    }
                }
            }
            if !resource_ref.contains_key("url") {
                bail!("github_repo: --url is required (no existing url to merge with)");
            }
            Ok(Some(Value::Object(resource_ref)))
        }
        "local_directory" => {
            if args.local_path.is_none()
                && args.daemon_id.is_none()
                && args.ref_label.is_none()
                && args.execution_mode.is_none()
            {
                return Ok(None);
            }
            let mut resource_ref = seed_resource_ref(
                existing,
                &["local_path", "daemon_id", "label", "execution_mode"],
            );
            for (flag, key, value) in [
                ("--local-path", "local_path", args.local_path.as_deref()),
                ("--daemon-id", "daemon_id", args.daemon_id.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        bail!("{flag} cannot be empty");
                    }
                    resource_ref.insert(key.into(), Value::String(value.into()));
                }
            }
            for (key, value) in [
                ("label", args.ref_label.as_deref()),
                ("execution_mode", args.execution_mode.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        resource_ref.remove(key);
                    } else {
                        resource_ref.insert(key.into(), Value::String(value.into()));
                    }
                }
            }
            if !resource_ref.contains_key("local_path") {
                bail!("local_directory: --local-path is required (no existing local_path to merge with)");
            }
            if !resource_ref.contains_key("daemon_id") {
                bail!("local_directory: --daemon-id is required (no existing daemon_id to merge with)");
            }
            Ok(Some(Value::Object(resource_ref)))
        }
        _ => {
            if args.url.is_some()
                || args.default_branch_hint.is_some()
                || args.local_path.is_some()
                || args.daemon_id.is_some()
                || args.ref_label.is_some()
                || args.execution_mode.is_some()
            {
                bail!(
                    "no built-in shortcut for resource type {resource_type:?}; pass the full payload via --ref '<json>'"
                );
            }
            Ok(None)
        }
    }
}

fn find_project_resource<'a>(resources: &'a [Value], resource_id: &str) -> Option<&'a Value> {
    resources
        .iter()
        .find(|resource| value_string(resource, "id") == resource_id)
}

async fn run_project_resource_update(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectResourceUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, &args.project_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let (resource_id, _) =
        resolve_project_resource_reference(&client, &project_id, &args.resource_id)
            .await
            .map_err(|error| anyhow::anyhow!("resolve project resource: {error}"))?;
    let existing: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("list project resources")?;
    let existing = find_project_resource(project_resources(&existing), &resource_id);
    let resource_type = existing
        .map(|resource| value_string(resource, "resource_type"))
        .unwrap_or_default();
    let existing_ref = existing
        .and_then(|resource| resource.get("resource_ref"))
        .and_then(Value::as_object);
    let mut body = serde_json::Map::new();
    if let Some(resource_ref) =
        build_project_resource_update_ref(args, &resource_type, existing_ref)?
    {
        body.insert("resource_ref".into(), resource_ref);
    }
    if args.clear_label {
        body.insert("label".into(), Value::Null);
    } else if let Some(label) = &args.label {
        body.insert("label".into(), Value::String(label.clone()));
    }
    if let Some(position) = args.position {
        body.insert("position".into(), Value::from(position));
    }
    if body.is_empty() {
        bail!(
            "nothing to update — pass --ref / --url / --local-path / --label / --position / --clear-label"
        );
    }
    let resource: Value = client
        .put_json(
            &format!("/api/projects/{project_id}/resources/{resource_id}"),
            &body,
        )
        .await
        .context("update project resource")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&resource)?),
            OutputFormat::Table => format_table(&[
                vec!["ID".into(), "TYPE".into(), "REF".into(), "LABEL".into()],
                vec![
                    value_string(&resource, "id"),
                    value_string(&resource, "resource_type"),
                    summarize_project_resource_ref(
                        resource.get("resource_ref").unwrap_or(&Value::Null),
                    ),
                    value_string(&resource, "label"),
                ],
            ]),
        },
        stderr: String::new(),
    })
}

async fn run_project_resource_remove(
    cli: &Cli,
    environment: &Environment,
    project: &str,
    resource: &str,
    _output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (project_id, project_display) = resolve_project_reference(&client, &workspace_id, project)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let (resource_id, resource_display) =
        resolve_project_resource_reference(&client, &project_id, resource)
            .await
            .map_err(|error| anyhow::anyhow!("resolve project resource: {error}"))?;
    client
        .delete(&format!(
            "/api/projects/{project_id}/resources/{resource_id}"
        ))
        .await
        .context("remove project resource")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Resource {resource_display} removed from project {project_display}.\n"),
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
                    "assignee_type":details.get(format!("{prefix}_type")).cloned().unwrap_or(Value::Null),
                    "assignee_id":details.get(format!("{prefix}_id")).cloned().unwrap_or(Value::Null),
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PropertyOption {
    id: String,
    name: String,
    #[serde(default)]
    color: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PropertyConfig {
    #[serde(default)]
    options: Vec<PropertyOption>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PropertyDefinition {
    id: String,
    name: String,
    #[serde(rename = "type")]
    property_type: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    config: PropertyConfig,
    #[serde(default)]
    position: f64,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    usage_count: i64,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
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
    list_property_definitions(client, true).await
}

async fn list_property_definitions(
    client: &ApiClient,
    include_archived: bool,
) -> Result<Vec<PropertyDefinition>> {
    let path = if include_archived {
        "/api/properties?include_archived=true"
    } else {
        "/api/properties"
    };
    let result: Value = client.get_json(path).await.context("list properties")?;
    serde_json::from_value(
        result
            .get("properties")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .context("decode properties")
}

fn format_property_definitions(
    properties: &[PropertyDefinition],
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(properties)?)),
        OutputFormat::Table => {
            let mut rows = vec![vec![
                "ID".into(),
                "ICON".into(),
                "NAME".into(),
                "TYPE".into(),
                "OPTIONS".into(),
                "USED".into(),
                "ARCHIVED".into(),
            ]];
            rows.extend(properties.iter().map(|property| {
                vec![
                    property.id.clone(),
                    property.icon.clone(),
                    property.name.clone(),
                    property.property_type.clone(),
                    property
                        .config
                        .options
                        .iter()
                        .map(|option| option.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    property.usage_count.to_string(),
                    if property.archived {
                        "yes".into()
                    } else {
                        String::new()
                    },
                ]
            }));
            Ok(format_table(&rows))
        }
    }
}

async fn run_property_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    include_archived: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = list_property_definitions(&client, include_archived).await?;
    Ok(RunOutput {
        stdout: format_property_definitions(&properties, output)?,
        stderr: String::new(),
    })
}

async fn run_property_get(
    cli: &Cli,
    environment: &Environment,
    property: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, property)?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(property)?),
            OutputFormat::Table => {
                format_property_definitions(std::slice::from_ref(property), output)?
            }
        },
        stderr: String::new(),
    })
}

const DEFAULT_PROPERTY_OPTION_COLOR: &str = "#6b7280";

fn parse_property_options(flags: &[String], existing: &[PropertyOption]) -> Vec<Value> {
    let by_name = existing
        .iter()
        .map(|option| (option.name.to_ascii_lowercase(), option.id.as_str()))
        .collect::<HashMap<_, _>>();
    flags
        .iter()
        .map(|raw| {
            let (name, color) = raw.rfind(":#").filter(|index| *index > 0).map_or_else(
                || (raw.as_str(), DEFAULT_PROPERTY_OPTION_COLOR),
                |index| (&raw[..index], &raw[index + 1..]),
            );
            let name = name.trim();
            let mut option = serde_json::Map::from_iter([
                ("name".into(), Value::String(name.into())),
                ("color".into(), Value::String(color.into())),
            ]);
            if let Some(id) = by_name.get(&name.to_ascii_lowercase()) {
                option.insert("id".into(), Value::String((*id).into()));
            }
            Value::Object(option)
        })
        .collect()
}

fn format_property_mutation(
    property: &PropertyDefinition,
    output: OutputFormat,
    action: &str,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(property)?)),
        OutputFormat::Table => Ok(format!(
            "Property {:?} {action}.\n{}",
            property.name,
            format_property_definitions(std::slice::from_ref(property), OutputFormat::Table)?
        )),
    }
}

async fn run_property_create(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyCreateArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let property_type = args
        .property_type
        .as_deref()
        .filter(|property_type| !property_type.is_empty())
        .context("--type is required")?;
    let mut body = serde_json::Map::from_iter([
        ("name".into(), Value::String(name.into())),
        ("type".into(), Value::String(property_type.into())),
        (
            "description".into(),
            Value::String(args.description.clone()),
        ),
        ("icon".into(), Value::String(args.icon.clone())),
    ]);
    if !args.option.is_empty() {
        body.insert(
            "config".into(),
            serde_json::json!({"options":parse_property_options(&args.option, &[])}),
        );
    }
    let client = new_api_client(cli, environment)?;
    let property: PropertyDefinition = client
        .post_json("/api/properties", &body)
        .await
        .context("create property")?;
    Ok(RunOutput {
        stdout: format_property_mutation(&property, args.output, "created")?,
        stderr: String::new(),
    })
}

async fn run_property_update(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, &args.property)?;
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("name", args.name.as_ref()),
        ("description", args.description.as_ref()),
        ("icon", args.icon.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if !args.option.is_empty() {
        body.insert(
            "config".into(),
            serde_json::json!({
                "options":parse_property_options(&args.option, &property.config.options)
            }),
        );
    }
    if body.is_empty() {
        bail!("nothing to update; pass --name, --description, --icon, or --option");
    }
    let updated: PropertyDefinition = client
        .patch_json(&format!("/api/properties/{}", property.id), &body)
        .await
        .context("update property")?;
    Ok(RunOutput {
        stdout: format_property_mutation(&updated, args.output, "updated")?,
        stderr: String::new(),
    })
}

async fn run_property_archive(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyArchiveArgs,
    archive: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, &args.property)?;
    let action = if archive { "archive" } else { "unarchive" };
    let updated: PropertyDefinition = client
        .patch_json(
            &format!("/api/properties/{}", property.id),
            &serde_json::json!({"archived":archive}),
        )
        .await
        .with_context(|| format!("{action} property"))?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&updated)?),
            OutputFormat::Table => format!(
                "Property {:?} {}.\n",
                updated.name,
                if archive { "archived" } else { "restored" }
            ),
        },
        stderr: String::new(),
    })
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

fn format_workspace_members(members: &[Value]) -> String {
    let mut rows = vec![vec![
        "USER ID".into(),
        "NAME".into(),
        "EMAIL".into(),
        "ROLE".into(),
    ]];
    rows.extend(members.iter().map(|member| {
        vec![
            value_string(member, "user_id"),
            value_string(member, "name"),
            value_string(member, "email"),
            value_string(member, "role"),
        ]
    }));
    format_table(&rows)
}

async fn run_workspace_member_list(
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
    let members: Vec<Value> = client
        .get_json(&format!("/api/workspaces/{workspace_id}/members"))
        .await
        .context("list members")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&members)?),
            OutputFormat::Table => format_workspace_members(&members),
        },
        stderr: String::new(),
    })
}

async fn run_workspace_switch(
    cli: &Cli,
    environment: &Environment,
    workspace: &str,
) -> Result<RunOutput> {
    require_human_local_command(environment, "workspace switch")?;
    let target = workspace.trim();
    if target.is_empty() {
        bail!("workspace id, slug, or id prefix is required");
    }
    let workspaces = fetch_workspaces(cli, environment).await?;
    let workspace = resolve_workspace_reference(&workspaces, target)?;
    environment.set_profile_value(
        &cli.profile,
        "workspace_id",
        Some(Value::String(workspace.id.clone())),
    )?;
    Ok(RunOutput {
        stdout: format!(
            "Switched to workspace: {} ({})\n",
            workspace.name, workspace.id
        ),
        stderr: String::new(),
    })
}

fn normalize_workspace_invite_role(role: &str) -> Result<String> {
    let role = match role.trim().to_ascii_lowercase() {
        role if role.is_empty() => "member".into(),
        role => role,
    };
    match role.as_str() {
        "member" | "admin" => Ok(role),
        "owner" => bail!("cannot invite as owner; use --role member or --role admin"),
        _ => bail!("invalid --role {role:?}; expected member or admin"),
    }
}

async fn run_workspace_member_invite(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceMemberInviteArgs,
) -> Result<RunOutput> {
    let email = args.email.trim().to_ascii_lowercase();
    if email.is_empty() {
        bail!("email is required");
    }
    let role = normalize_workspace_invite_role(&args.role)?;
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    let invitation: Value = client
        .post_json(
            &format!("/api/workspaces/{workspace_id}/members"),
            &serde_json::json!({"email":email,"role":role}),
        )
        .await
        .context("invite member")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&invitation)?),
            OutputFormat::Table => format!(
                "Invitation sent to {} (role: {}, status: {})\n",
                value_string(&invitation, "invitee_email"),
                value_string(&invitation, "role"),
                value_string(&invitation, "status")
            ),
        },
        stderr: String::new(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceMcpServer {
    id: String,
    name: String,
    transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
}

fn format_workspace_mcp_servers(
    servers: &[WorkspaceMcpServer],
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(servers)?)),
        OutputFormat::Table if servers.is_empty() => Ok("no MCP servers\n".into()),
        OutputFormat::Table => {
            let mut rows = vec![vec![
                "ID".into(),
                "NAME".into(),
                "TRANSPORT".into(),
                "STATUS".into(),
            ]];
            rows.extend(servers.iter().map(|server| {
                vec![
                    server.id.clone(),
                    server.name.clone(),
                    server.transport.clone(),
                    server.enabled.map_or_else(String::new, |enabled| {
                        if enabled { "enabled" } else { "disabled" }.into()
                    }),
                ]
            }));
            Ok(format_table(&rows))
        }
    }
}

async fn run_workspace_mcp_list(
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
    let servers: Vec<WorkspaceMcpServer> = client
        .get_json(&format!("/api/workspaces/{workspace_id}/mcp-servers"))
        .await
        .context("list workspace mcp servers")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(&servers, output)?,
        stderr: String::new(),
    })
}

fn parse_workspace_mcp_server_config(raw: &str) -> Result<Value> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("--server-config: empty input; pass a JSON object");
    }
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| anyhow::anyhow!("--server-config must be a valid JSON object"))?;
    match &value {
        Value::Null => bail!("--server-config must be a JSON object, not null"),
        Value::Object(_) => Ok(value),
        _ => bail!("--server-config must be a JSON object"),
    }
}

fn resolve_workspace_mcp_server_config<R: Read>(
    inline: Option<&str>,
    from_stdin: bool,
    file: Option<&Path>,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<Value>> {
    let count = [inline.is_some(), from_stdin, file.is_some()]
        .into_iter()
        .filter(|source| *source)
        .count();
    if count > 1 {
        bail!(
            "--server-config, --server-config-stdin, and --server-config-file are mutually exclusive; pick one"
        );
    }
    let raw = if let Some(inline) = inline {
        inline.into()
    } else if from_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read --server-config-stdin")?;
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if raw.trim().is_empty() {
            bail!("--server-config-stdin: empty input");
        }
        raw
    } else if let Some(file) = file {
        if file.as_os_str().is_empty() {
            bail!("--server-config-file: path must not be empty");
        }
        let path = if file.is_absolute() {
            file.to_path_buf()
        } else {
            environment.current_dir().join(file)
        };
        let bytes = fs::read(&path).context("read --server-config-file")?;
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if raw.trim().is_empty() {
            bail!(
                "--server-config-file {:?}: empty contents",
                file.to_string_lossy()
            );
        }
        raw
    } else {
        return Ok(None);
    };
    parse_workspace_mcp_server_config(&raw).map(Some)
}

async fn run_workspace_mcp_add<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceMcpAddArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let server_name = args.server_name.trim();
    if server_name.is_empty() {
        bail!("server name must not be empty");
    }
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let config = resolve_workspace_mcp_server_config(
        args.server_config.as_deref(),
        args.server_config_stdin,
        args.server_config_file.as_deref(),
        environment,
        input,
    )?
    .context(
        "one of --server-config, --server-config-stdin, or --server-config-file is required",
    )?;
    let client = new_api_client(cli, environment)?;
    let server: WorkspaceMcpServer = client
        .post_json(
            &format!("/api/workspaces/{workspace_id}/mcp-servers"),
            &serde_json::json!({"name":server_name,"config":config}),
        )
        .await
        .context("add workspace mcp server")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(std::slice::from_ref(&server), args.output)?,
        stderr: String::new(),
    })
}

fn encoded_path_segment(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn run_workspace_mcp_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &WorkspaceMcpUpdateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let server_id = args.server_id.trim();
    if server_id.is_empty() {
        bail!("server ID must not be empty");
    }
    let workspace_id = resolve_workspace_arg(cli, environment, args.workspace.as_deref()).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let mut body = serde_json::Map::new();
    if let Some(name) = &args.name {
        body.insert("name".into(), Value::String(name.trim().into()));
    }
    if let Some(config) = resolve_workspace_mcp_server_config(
        args.server_config.as_deref(),
        args.server_config_stdin,
        args.server_config_file.as_deref(),
        environment,
        input,
    )? {
        body.insert("config".into(), config);
    }
    if body.is_empty() {
        bail!(
            "nothing to update; pass --name and/or one of --server-config, --server-config-stdin, --server-config-file"
        );
    }
    let client = new_api_client(cli, environment)?;
    let server: WorkspaceMcpServer = client
        .put_json(
            &format!(
                "/api/workspaces/{workspace_id}/mcp-servers/{}",
                encoded_path_segment(server_id)
            ),
            &body,
        )
        .await
        .context("update workspace mcp server")?;
    Ok(RunOutput {
        stdout: format_workspace_mcp_servers(std::slice::from_ref(&server), args.output)?,
        stderr: String::new(),
    })
}

async fn run_workspace_mcp_remove(
    cli: &Cli,
    environment: &Environment,
    server_id: &str,
    workspace: Option<&str>,
    _output: OutputFormat,
) -> Result<RunOutput> {
    let server_id = server_id.trim();
    if server_id.is_empty() {
        bail!("server ID must not be empty");
    }
    let workspace_id = resolve_workspace_arg(cli, environment, workspace).await?;
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required: pass an id/slug/prefix as argument or set CORDY_WORKSPACE_ID"
        );
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!(
            "/api/workspaces/{workspace_id}/mcp-servers/{}",
            encoded_path_segment(server_id)
        ))
        .await
        .context("remove workspace mcp server")?;
    Ok(RunOutput {
        stdout: format!("removed MCP server {server_id}\n"),
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

    #[tokio::test]
    async fn private_execenv_helper_dispatches_before_cli_parsing() {
        let missing = tempfile::tempdir()
            .expect("tempdir")
            .path()
            .join("missing-workdir");
        let input = serde_json::to_vec(&serde_json::json!({
            "action": "reuse",
            "reuse": {
                "WorkDir": missing,
                "Provider": "codex"
            }
        }))
        .expect("helper request");
        let mut output = Vec::new();

        let handled = run_private_helper(
            &[
                OsString::from("cordy"),
                OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
            ],
            Cursor::new(input),
            &mut output,
        )
        .await
        .expect("private helper");

        assert!(handled);
        let response: Value = serde_json::from_slice(&output).expect("helper response");
        assert!(response.get("environment").is_none());
        assert!(response.get("error").is_none());
    }

    #[tokio::test]
    async fn private_execenv_helper_requires_the_exact_private_argv() {
        let mut output = Vec::new();
        let handled = run_private_helper(
            &[
                OsString::from("cordy"),
                OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG),
                OsString::from("unexpected"),
            ],
            Cursor::new(Vec::<u8>::new()),
            &mut output,
        )
        .await
        .expect("ordinary CLI path");

        assert!(!handled);
        assert!(output.is_empty());
    }

    #[test]
    fn autopilot_read_parser_matches_go_registry() {
        let list = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "list",
            "--status",
            "paused",
            "--output",
            "json",
            "--full-id",
        ])
        .expect("autopilot list CLI");
        let Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::List {
                    status,
                    output,
                    full_id,
                },
        }) = list.command
        else {
            panic!("expected autopilot list");
        };
        assert_eq!(status, "paused");
        assert_eq!(output, OutputFormat::Json);
        assert!(full_id);

        let get =
            Cli::try_parse_from(["cordy", "autopilot", "get", "abcd"]).expect("autopilot get CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Get { id, output },
        }) = get.command
        else {
            panic!("expected autopilot get");
        };
        assert_eq!(id, "abcd");

        let trigger =
            Cli::try_parse_from(["cordy", "autopilot", "trigger", "abcd", "--output", "table"])
                .expect("autopilot trigger CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Trigger { id, output },
        }) = trigger.command
        else {
            panic!("expected autopilot trigger");
        };
        assert_eq!(id, "abcd");
        assert_eq!(output, OutputFormat::Table);

        let runs = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "runs",
            "abcd",
            "--limit",
            "5",
            "--offset",
            "2",
            "--output",
            "json",
        ])
        .expect("autopilot runs CLI");
        let Command::Autopilot(AutopilotArgs {
            command:
                AutopilotCommand::Runs {
                    id,
                    limit,
                    offset,
                    output,
                },
        }) = runs.command
        else {
            panic!("expected autopilot runs");
        };
        assert_eq!(id, "abcd");
        assert_eq!(limit, 5);
        assert_eq!(offset, 2);
        assert_eq!(output, OutputFormat::Json);

        let add = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "trigger-add",
            "abcd",
            "--kind",
            "webhook",
            "--label",
            "GitHub",
        ])
        .expect("autopilot trigger-add CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerAdd(args),
        }) = add.command
        else {
            panic!("expected autopilot trigger-add");
        };
        assert_eq!(args.autopilot_id, "abcd");
        assert_eq!(args.kind, "webhook");
        assert_eq!(args.label, "GitHub");
        assert_eq!(args.output, OutputFormat::Json);

        let update = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "trigger-update",
            "abcd",
            "beef",
            "--enabled=false",
            "--cron=",
            "--label=",
        ])
        .expect("autopilot trigger-update CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::TriggerUpdate(args),
        }) = update.command
        else {
            panic!("expected autopilot trigger-update");
        };
        assert_eq!(args.autopilot_id, "abcd");
        assert_eq!(args.trigger_id, "beef");
        assert_eq!(args.enabled, Some(false));
        assert_eq!(args.cron.as_deref(), Some(""));
        assert_eq!(args.label.as_deref(), Some(""));

        let delete = Cli::try_parse_from(["cordy", "autopilot", "trigger-delete", "abcd", "beef"])
            .expect("autopilot trigger-delete CLI");
        assert!(matches!(
            delete.command,
            Command::Autopilot(AutopilotArgs {
                command: AutopilotCommand::TriggerDelete { .. }
            })
        ));
        assert_eq!(output, OutputFormat::Json);
        assert!(Cli::try_parse_from(["cordy", "autopilot", "get"]).is_err());
        assert!(Cli::try_parse_from(["cordy", "autopilot", "list", "extra"]).is_err());

        let create = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "create",
            "--title",
            "Daily planner",
            "--agent",
            "Planner",
            "--mode",
            "create_issue",
            "--priority",
            "high",
            "--subscriber",
            "Alice",
            "--subscriber",
            "Bob",
            "--output",
            "table",
        ])
        .expect("autopilot create CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Create(args),
        }) = create.command
        else {
            panic!("expected autopilot create");
        };
        assert_eq!(args.title.as_deref(), Some("Daily planner"));
        assert_eq!(args.agent.as_deref(), Some("Planner"));
        assert_eq!(args.mode.as_deref(), Some("create_issue"));
        assert_eq!(args.priority.as_deref(), Some("high"));
        assert_eq!(args.subscriber, ["Alice", "Bob"]);
        assert_eq!(args.output, OutputFormat::Table);

        let update = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "update",
            "abcd",
            "--project=",
            "--clear-subscribers",
        ])
        .expect("autopilot update CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Update(args),
        }) = update.command
        else {
            panic!("expected autopilot update");
        };
        assert_eq!(args.id, "abcd");
        assert_eq!(args.project.as_deref(), Some(""));
        assert!(args.clear_subscribers);
        assert_eq!(args.output, OutputFormat::Json);

        let delete = Cli::try_parse_from(["cordy", "autopilot", "delete", "abcd"])
            .expect("autopilot delete CLI");
        let Command::Autopilot(AutopilotArgs {
            command: AutopilotCommand::Delete { id },
        }) = delete.command
        else {
            panic!("expected autopilot delete");
        };
        assert_eq!(id, "abcd");
    }

    #[tokio::test]
    async fn autopilot_create_resolves_references_and_preserves_go_body() {
        const AGENT_ID: &str = "11111111-1111-1111-1111-111111111111";
        const PROJECT_ID: &str = "22222222-2222-2222-2222-222222222222";
        const USER_ID: &str = "33333333-3333-3333-3333-333333333333";
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/agents",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                    Json(vec![
                        serde_json::json!({"id":AGENT_ID,"name":"Daily Planner"}),
                    ])
                }),
            )
            .route(
                "/api/projects",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                    Json(serde_json::json!({
                        "projects":[{"id":PROJECT_ID,"title":"Operations","status":"planned"}]
                    }))
                }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "user_id":USER_ID,
                        "name":"Alice",
                        "email":"alice@example.com"
                    })])
                }),
            )
            .route(
                "/api/autopilots",
                post(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_handler);
                    async move {
                        *captured.lock().expect("captured body") = Some(body.clone());
                        Json(serde_json::json!({
                            "id":"autopilot-1",
                            "title":body["title"],
                            "server_only":"preserved"
                        }))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "create",
            "--title",
            "Daily planner",
            "--description",
            "Plan each day",
            "--agent",
            "planner",
            "--mode",
            "create_issue",
            "--priority",
            "high",
            "--project",
            "2222",
            "--issue-title-template",
            "Daily {{date}}",
            "--subscriber",
            "Alice",
            "--subscriber",
            "alice@example.com",
            "--output",
            "table",
        ])
        .expect("autopilot create CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create autopilot");
        assert_eq!(
            output.stdout,
            "Autopilot created: Daily planner (autopilot-1)\n"
        );
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("request body");
        assert_eq!(body["title"], "Daily planner");
        assert_eq!(body["description"], "Plan each day");
        assert_eq!(body["assignee_id"], AGENT_ID);
        assert_eq!(body["execution_mode"], "create_issue");
        assert_eq!(body["priority"], "high");
        assert_eq!(body["project_id"], PROJECT_ID);
        assert_eq!(body["issue_title_template"], "Daily {{date}}");
        assert_eq!(
            body["subscribers"],
            serde_json::json!([{"user_type":"member","user_id":USER_ID}])
        );
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_create_rejects_missing_and_invalid_required_values() {
        const AGENT_ID: &str = "11111111-1111-1111-1111-111111111111";
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");

        for (argv, expected) in [
            (vec!["cordy", "autopilot", "create"], "--title is required"),
            (
                vec!["cordy", "autopilot", "create", "--title", "Daily"],
                "--agent is required (agent name or ID)",
            ),
            (
                vec![
                    "cordy",
                    "autopilot",
                    "create",
                    "--title",
                    "Daily",
                    "--agent",
                    AGENT_ID,
                ],
                "--mode is required (create_issue or run_only)",
            ),
            (
                vec![
                    "cordy",
                    "autopilot",
                    "create",
                    "--title",
                    "Daily",
                    "--agent",
                    AGENT_ID,
                    "--mode",
                    "invalid",
                ],
                "--mode must be create_issue or run_only",
            ),
        ] {
            let cli = Cli::try_parse_from(argv).expect("autopilot create CLI");
            let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect_err("invalid create rejected");
            assert_eq!(error.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn autopilot_update_resolves_references_and_patches_only_changed_fields() {
        const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        const AGENT_ID: &str = "11111111-1111-1111-1111-111111111111";
        const PROJECT_ID: &str = "22222222-2222-2222-2222-222222222222";
        const USER_ID: &str = "33333333-3333-3333-3333-333333333333";
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/agents",
                get(|| async {
                    Json(vec![
                        serde_json::json!({"id":AGENT_ID,"name":"Codex Agent"}),
                    ])
                }),
            )
            .route(
                "/api/projects",
                get(|| async {
                    Json(serde_json::json!({"projects":[{"id":PROJECT_ID,"title":"Ops"}]}))
                }),
            )
            .route(
                "/api/workspaces/workspace-1/members",
                get(|| async { Json(vec![serde_json::json!({"user_id":USER_ID,"name":"Alice"})]) }),
            )
            .route(
                "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                patch(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_handler);
                    async move {
                        *captured.lock().expect("captured body") = Some(body);
                        Json(serde_json::json!({"id":AUTOPILOT_ID,"title":"Updated"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "update",
            AUTOPILOT_ID,
            "--title",
            "Updated",
            "--description=",
            "--agent",
            "Codex",
            "--project",
            "2222",
            "--priority",
            "urgent",
            "--status",
            "paused",
            "--mode",
            "run_only",
            "--issue-title-template=",
            "--subscriber",
            "Alice",
            "--output",
            "table",
        ])
        .expect("autopilot update CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update autopilot");
        assert_eq!(
            output.stdout,
            format!("Autopilot updated: Updated ({AUTOPILOT_ID})\n")
        );
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("request body");
        assert_eq!(body["title"], "Updated");
        assert_eq!(body["description"], "");
        assert_eq!(body["assignee_type"], "agent");
        assert_eq!(body["assignee_id"], AGENT_ID);
        assert_eq!(body["project_id"], PROJECT_ID);
        assert_eq!(body["priority"], "urgent");
        assert_eq!(body["status"], "paused");
        assert_eq!(body["execution_mode"], "run_only");
        assert_eq!(body["issue_title_template"], "");
        assert_eq!(
            body["subscribers"],
            serde_json::json!([{"user_type":"member","user_id":USER_ID}])
        );
        assert_eq!(body.as_object().map(serde_json::Map::len), Some(10));
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_update_preserves_clear_and_no_change_semantics() {
        const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            patch(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body);
                    Json(serde_json::json!({"id":ID,"title":"Daily"}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        let clear = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "update",
            ID,
            "--project=",
            "--clear-subscribers",
        ])
        .expect("autopilot clear CLI");
        run_with_input(&clear, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("clear autopilot fields");
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("request body");
        assert!(body["project_id"].is_null());
        assert_eq!(body["subscribers"], serde_json::json!([]));

        let no_change = Cli::try_parse_from(["cordy", "autopilot", "update", ID])
            .expect("autopilot no-change CLI");
        let error = run_with_input(&no_change, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("no-change update rejected");
        assert_eq!(
            error.to_string(),
            "no fields to update; use flags like --title, --description, --agent, --status, --mode, etc."
        );

        let conflict = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "update",
            ID,
            "--subscriber",
            "Alice",
            "--clear-subscribers",
        ])
        .expect("autopilot subscriber conflict CLI");
        let error = run_with_input(&conflict, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("subscriber conflict rejected");
        assert_eq!(
            error.to_string(),
            "--subscriber and --clear-subscribers are mutually exclusive"
        );
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_delete_resolves_prefix_and_reports_title() {
        const ID: &str = "abcd0000-1111-2222-3333-444444444444";
        let app = Router::new()
            .route(
                "/api/autopilots",
                get(|request: Request| async move {
                    assert_eq!(
                        request.uri().query(),
                        Some("limit=50&workspace_id=workspace-1")
                    );
                    Json(serde_json::json!({
                        "autopilots":[{"id":ID,"title":"Daily planner","status":"active"}],
                        "total":1
                    }))
                }),
            )
            .route(
                "/api/autopilots/abcd0000-1111-2222-3333-444444444444",
                delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        let cli = Cli::try_parse_from(["cordy", "autopilot", "delete", "abcd"])
            .expect("autopilot delete CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete autopilot");
        assert_eq!(output.stdout, "Autopilot Daily planner deleted.\n");
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_trigger_and_runs_match_go_requests_and_outputs() {
        const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let app = Router::new()
            .route(
                "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/trigger",
                post(|Json(body): Json<Value>| async move {
                    assert!(body.is_null());
                    Json(serde_json::json!({
                        "id":"run-1",
                        "status":"queued",
                        "server_only":"preserved"
                    }))
                }),
            )
            .route(
                "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/runs",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("limit=5&offset=2"));
                    Json(serde_json::json!({
                        "runs":[{
                            "id":"run-1",
                            "source":"manual",
                            "status":"completed",
                            "issue_id":"issue-1",
                            "triggered_at":"2026-08-24T01:00:00Z",
                            "completed_at":"2026-08-24T01:01:00Z",
                            "server_only":"preserved"
                        }],
                        "total":1
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");

        let trigger =
            Cli::try_parse_from(["cordy", "autopilot", "trigger", ID, "--output", "table"])
                .expect("autopilot trigger CLI");
        let output = run_with_input(&trigger, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("trigger autopilot");
        assert_eq!(
            output.stdout,
            "Autopilot triggered: run run-1 (status: queued)\n"
        );

        let runs = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "runs",
            ID,
            "--limit",
            "5",
            "--offset",
            "2",
        ])
        .expect("autopilot runs table CLI");
        let output = run_with_input(&runs, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list autopilot runs");
        assert!(output.stdout.starts_with("ID"));
        assert!(output.stdout.contains("run-1"));
        assert!(output.stdout.contains("manual"));
        assert!(output.stdout.contains("issue-1"));

        let runs_json = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "runs",
            ID,
            "--limit",
            "5",
            "--offset",
            "2",
            "--output",
            "json",
        ])
        .expect("autopilot runs JSON CLI");
        let output = run_with_input(&runs_json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list autopilot runs JSON");
        let value: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(value["total"], 1);
        assert_eq!(value["runs"][0]["server_only"], "preserved");
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_trigger_add_preserves_schedule_and_webhook_semantics() {
        const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/triggers",
            post(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    captured.lock().expect("captured bodies").push(body.clone());
                    if body["kind"] == "webhook" {
                        Json(serde_json::json!({
                            "id":"trigger-webhook",
                            "kind":"webhook",
                            "webhook_url":"https://hooks.example/direct",
                            "webhook_path":"/ignored"
                        }))
                    } else {
                        Json(serde_json::json!({"id":"trigger-schedule","kind":"schedule"}))
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}/"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");

        let schedule = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "trigger-add",
            ID,
            "--cron",
            "0 9 * * *",
            "--timezone",
            "America/New_York",
            "--label",
            "Morning",
        ])
        .expect("schedule trigger CLI");
        run_with_input(&schedule, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create schedule trigger");

        let webhook = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "trigger-add",
            ID,
            "--kind",
            "webhook",
            "--label",
            "GitHub",
            "--output",
            "table",
        ])
        .expect("webhook trigger CLI");
        let output = run_with_input(&webhook, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create webhook trigger");
        assert_eq!(
            output.stdout,
            "Trigger created: trigger-webhook (kind=webhook)\nWebhook URL: https://hooks.example/direct\n"
        );
        let bodies = captured.lock().expect("captured bodies");
        assert_eq!(bodies[0]["kind"], "schedule");
        assert_eq!(bodies[0]["cron_expression"], "0 9 * * *");
        assert_eq!(bodies[0]["timezone"], "America/New_York");
        assert_eq!(bodies[0]["label"], "Morning");
        assert_eq!(
            bodies[1],
            serde_json::json!({"kind":"webhook","label":"GitHub"})
        );
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_trigger_add_rejects_invalid_kind_specific_flags() {
        const ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        for (extra, expected) in [
            (
                vec!["--kind", "invalid"],
                "--kind must be schedule or webhook",
            ),
            (vec![], "--cron is required for --kind schedule"),
            (
                vec!["--kind", "webhook", "--timezone", "UTC"],
                "--timezone is only valid with --kind schedule",
            ),
            (
                vec!["--kind", "webhook", "--cron", "* * * * *"],
                "--cron is only valid with --kind schedule",
            ),
        ] {
            let mut argv = vec!["cordy", "autopilot", "trigger-add", ID];
            argv.extend(extra);
            let cli = Cli::try_parse_from(argv).expect("trigger-add CLI");
            let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect_err("invalid trigger rejected");
            assert_eq!(error.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn autopilot_trigger_update_and_delete_resolve_prefixes_and_mutate() {
        const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        const TRIGGER_ID: &str = "bbbb0000-1111-2222-3333-444444444444";
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                get(|| async {
                    Json(serde_json::json!({
                        "autopilot":{"id":AUTOPILOT_ID},
                        "triggers":[{"id":TRIGGER_ID,"kind":"schedule","label":"Morning"}]
                    }))
                }),
            )
            .route(
                "/api/autopilots/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/triggers/bbbb0000-1111-2222-3333-444444444444",
                patch(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_handler);
                    async move {
                        *captured.lock().expect("captured body") = Some(body);
                        Json(serde_json::json!({"id":TRIGGER_ID,"enabled":false}))
                    }
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");

        let update = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "trigger-update",
            AUTOPILOT_ID,
            "bbbb",
            "--enabled=false",
            "--cron=",
            "--timezone",
            "UTC",
            "--label=",
            "--output",
            "table",
        ])
        .expect("trigger-update CLI");
        let output = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update trigger");
        assert_eq!(output.stdout, format!("Trigger updated: {TRIGGER_ID}\n"));
        assert_eq!(
            captured.lock().expect("captured body").as_ref(),
            Some(&serde_json::json!({
                "enabled":false,
                "cron_expression":"",
                "timezone":"UTC",
                "label":""
            }))
        );

        let delete =
            Cli::try_parse_from(["cordy", "autopilot", "trigger-delete", AUTOPILOT_ID, "bbbb"])
                .expect("trigger-delete CLI");
        let output = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete trigger");
        assert_eq!(output.stdout, format!("Trigger {TRIGGER_ID} deleted.\n"));
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_trigger_update_rejects_no_changes_before_requests() {
        const AUTOPILOT_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        const TRIGGER_ID: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        let cli = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "trigger-update",
            AUTOPILOT_ID,
            TRIGGER_ID,
        ])
        .expect("trigger-update CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("no-change trigger rejected");
        assert_eq!(
            error.to_string(),
            "no fields to update; use --enabled, --cron, --timezone, or --label"
        );
    }

    #[tokio::test]
    async fn autopilot_list_matches_go_filter_actor_and_output_semantics() {
        let app = Router::new()
            .route(
                "/api/autopilots",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("status=paused"));
                    Json(serde_json::json!({
                        "autopilots":[{
                            "id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                            "title":"Nightly review",
                            "status":"paused",
                            "execution_mode":"run_only",
                            "assignee_id":"agent-1",
                            "last_run_at":"2026-08-24T01:02:03Z",
                            "server_only":"preserved"
                        }],
                        "total":1
                    }))
                }),
            )
            .route(
                "/api/agents",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("workspace_id=workspace-1"));
                    Json(vec![serde_json::json!({"id":"agent-1","name":"Reviewer"})])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "autopilot", "list", "--status", "paused"])
            .expect("autopilot list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list autopilots");
        assert!(output.stdout.starts_with("ID"));
        assert!(output.stdout.contains("aaaaaaaa"));
        assert!(!output.stdout.contains("aaaaaaaa-aaaa"));
        assert!(output.stdout.contains("Nightly review"));
        assert!(output.stdout.contains("Reviewer"));
        assert!(output.stdout.contains("2026-08-24T01:02:03Z"));

        let json = Cli::try_parse_from([
            "cordy",
            "autopilot",
            "list",
            "--status",
            "paused",
            "--output",
            "json",
        ])
        .expect("autopilot list JSON CLI");
        let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list autopilots as JSON");
        let value: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(value["total"], 1);
        assert_eq!(value["autopilots"][0]["server_only"], "preserved");
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_get_resolves_prefix_and_preserves_detail_envelope() {
        const ID: &str = "abcd0000-1111-2222-3333-444444444444";
        let app = Router::new()
            .route(
                "/api/autopilots",
                get(|request: Request| async move {
                    match request.uri().query() {
                        Some("limit=50&workspace_id=workspace-1") => Json(serde_json::json!({
                            "autopilots":(0..50).map(|index| serde_json::json!({
                                "id":format!("{index:08x}-1111-2222-3333-444444444444")
                            })).collect::<Vec<_>>(),
                            "total":51,
                            "has_more":true
                        })),
                        Some("limit=50&offset=50&workspace_id=workspace-1") => {
                            Json(serde_json::json!({
                                "autopilots":[{"id":ID,"title":"Morning triage","status":"active"}],
                                "total":51,
                                "has_more":false
                            }))
                        }
                        query => panic!("unexpected resolver query: {query:?}"),
                    }
                }),
            )
            .route(
                "/api/autopilots/abcd0000-1111-2222-3333-444444444444",
                get(|| async {
                    Json(serde_json::json!({
                        "autopilot":{
                            "id":ID,
                            "title":"Morning triage",
                            "status":"active",
                            "execution_mode":"create_issue",
                            "assignee_id":"agent-1",
                            "last_run_at":null
                        },
                        "triggers":[{"id":"trigger-1","kind":"schedule"}],
                        "collaborators":[],
                        "server_only":"preserved"
                    }))
                }),
            )
            .route(
                "/api/agents",
                get(|| async { Json(vec![serde_json::json!({"id":"agent-1","name":"Planner"})]) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let table = Cli::try_parse_from(["cordy", "autopilot", "get", "abcd", "--output", "table"])
            .expect("autopilot get table CLI");
        let output = run_with_input(&table, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get autopilot table");
        assert!(output.stdout.contains(ID));
        assert!(output.stdout.contains("Planner"));

        let json =
            Cli::try_parse_from(["cordy", "autopilot", "get", ID]).expect("autopilot get JSON CLI");
        let output = run_with_input(&json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get autopilot JSON");
        let value: Value = serde_json::from_str(&output.stdout).expect("JSON output");
        assert_eq!(value["triggers"][0]["kind"], "schedule");
        assert_eq!(value["server_only"], "preserved");
        server.abort();
    }

    #[tokio::test]
    async fn autopilot_prefix_errors_match_go_resolver_contract() {
        let app = Router::new().route(
            "/api/autopilots",
            get(|| async {
                Json(serde_json::json!({
                    "autopilots":[
                        {"id":"abcd0000-1111-2222-3333-444444444444"},
                        {"id":"abcd9999-1111-2222-3333-444444444444"}
                    ],
                    "total":2
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");

        let short =
            Cli::try_parse_from(["cordy", "autopilot", "get", "abc"]).expect("short prefix CLI");
        let error = run_with_input(&short, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("short prefix rejected");
        assert_eq!(
            error.to_string(),
            "resolve autopilot: resolve autopilot: expected a full UUID or at least 4 hex characters, got \"abc\""
        );

        let ambiguous = Cli::try_parse_from(["cordy", "autopilot", "get", "abcd"])
            .expect("ambiguous prefix CLI");
        let error = run_with_input(&ambiguous, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("ambiguous prefix rejected");
        assert!(error
            .to_string()
            .starts_with("ambiguous autopilot id prefix \"abcd\"; matches:"));
        assert!(error
            .to_string()
            .contains("abcd0000-1111-2222-3333-444444444444"));
        assert!(error
            .to_string()
            .contains("abcd9999-1111-2222-3333-444444444444"));
        server.abort();
    }

    #[test]
    fn agent_read_parser_matches_go_registry() {
        let list = Cli::try_parse_from([
            "cordy",
            "agent",
            "list",
            "--include-archived",
            "--output",
            "json",
        ])
        .expect("agent list CLI");
        let Command::Agent(AgentArgs {
            command:
                AgentCommand::List {
                    output,
                    include_archived,
                },
        }) = list.command
        else {
            panic!("expected agent list");
        };
        assert_eq!(output, OutputFormat::Json);
        assert!(include_archived);

        let get =
            Cli::try_parse_from(["cordy", "agent", "get", "agent-123"]).expect("agent get CLI");
        let Command::Agent(AgentArgs {
            command: AgentCommand::Get { id, output },
        }) = get.command
        else {
            panic!("expected agent get");
        };
        assert_eq!(id, "agent-123");
        assert_eq!(output, OutputFormat::Json);
        assert!(Cli::try_parse_from(["cordy", "agent", "list", "--full-id"]).is_err());
    }

    #[tokio::test]
    async fn agent_list_and_get_match_go_requests_and_outputs() {
        let app = Router::new()
            .route(
                "/api/agents",
                get(|request: Request| async move {
                    assert_eq!(
                        request.uri().query(),
                        Some("workspace_id=workspace-1&include_archived=true")
                    );
                    Json(vec![serde_json::json!({
                        "id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                        "name":"Builder",
                        "status":"active",
                        "runtime_mode":"cloud",
                        "archived_at":"2026-08-24T00:00:00Z",
                        "server_only":"preserved"
                    })])
                }),
            )
            .route(
                "/api/agents/agent-123",
                get(|| async {
                    Json(serde_json::json!({
                        "id":"agent-123",
                        "name":"Reviewer",
                        "status":"idle",
                        "runtime_mode":"local",
                        "visibility":"workspace",
                        "avatar_url":"https://cdn.example/avatar.png",
                        "description":"Reviews changes",
                        "server_only":"preserved"
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let list = Cli::try_parse_from([
            "cordy",
            "agent",
            "list",
            "--include-archived",
            "--output",
            "table",
        ])
        .expect("agent list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list agents");
        assert!(listed.stdout.starts_with("ID"));
        assert!(listed
            .stdout
            .contains("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"));
        assert!(listed.stdout.contains("Builder"));
        assert!(listed.stdout.contains("cloud"));
        assert!(listed.stdout.contains("yes"));

        let get = Cli::try_parse_from(["cordy", "agent", "get", "agent-123", "--output", "table"])
            .expect("agent get CLI");
        let details = run_with_input(&get, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get agent");
        assert!(details.stdout.contains("AVATAR_URL"));
        assert!(details.stdout.contains("https://cdn.example/avatar.png"));
        assert!(details.stdout.contains("Reviews changes"));

        let get_json = Cli::try_parse_from(["cordy", "agent", "get", "agent-123"])
            .expect("agent get JSON CLI");
        let json = run_with_input(&get_json, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get agent JSON");
        assert_eq!(
            serde_json::from_str::<Value>(&json.stdout).expect("JSON")["server_only"],
            "preserved"
        );
        server.abort();
    }

    #[tokio::test]
    async fn agent_list_requires_workspace_before_request() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "agent", "list"]).expect("agent list CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("workspace required");
        assert_eq!(
            error.to_string(),
            "workspace_id is required: use --workspace-id flag, set CORDY_WORKSPACE_ID env, or run 'cordy config set workspace_id <id>'"
        );
    }

    #[tokio::test]
    async fn agent_create_preserves_go_request_and_secret_input_semantics() {
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/agents",
            post(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body);
                    Json(serde_json::json!({"id":"agent-1","name":"Builder"}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("agent.env.json"), r#"{"TOKEN":"secret"}"#).expect("env file");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "agent",
            "create",
            "--name",
            "Builder",
            "--runtime-id",
            "runtime-1",
            "--description",
            "Builds things",
            "--instructions",
            "Be careful",
            "--runtime-config",
            r#"{"sandbox":true}"#,
            "--custom-args",
            r#"["--model","fast"]"#,
            "--custom-env-file",
            "agent.env.json",
            "--mcp-config-stdin",
            "--model",
            "model-1",
            "--thinking-level",
            "high",
            "--service-tier",
            "priority",
            "--visibility",
            "workspace",
            "--public-to-workspace",
            "--public-to-member",
            "user-1,user-2",
            "--max-concurrent-tasks",
            "50",
            "--output",
            "table",
        ])
        .expect("agent create CLI");
        let mut input = Cursor::new(br#"{"mcpServers":{"linear":{"token":"hidden"}}}"#.to_vec());
        let output = run_with_input(&cli, &environment, &mut input)
            .await
            .expect("create agent");
        assert_eq!(output.stdout, "Agent created: Builder (agent-1)\n");
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("request body");
        assert_eq!(body["name"], "Builder");
        assert_eq!(body["runtime_id"], "runtime-1");
        assert_eq!(body["runtime_config"]["sandbox"], true);
        assert_eq!(body["custom_args"], serde_json::json!(["--model", "fast"]));
        assert_eq!(body["custom_env"]["TOKEN"], "secret");
        assert_eq!(
            body["mcp_config"]["mcpServers"]["linear"]["token"],
            "hidden"
        );
        assert_eq!(body["model"], "model-1");
        assert_eq!(body["thinking_level"], "high");
        assert_eq!(body["service_tier"], "priority");
        assert_eq!(body["visibility"], "workspace");
        assert_eq!(body["permission_mode"], "public_to");
        assert_eq!(
            body["invocation_targets"],
            serde_json::json!([
                {"target_type":"workspace"},
                {"target_type":"member","target_id":"user-1"},
                {"target_type":"member","target_id":"user-2"}
            ])
        );
        assert_eq!(body["max_concurrent_tasks"], 50);
        server.abort();
    }

    #[test]
    fn agent_create_rejects_invalid_and_ambiguous_secret_inputs_without_leaking() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let secret = "sk-do-not-echo";
        let invalid = Cli::try_parse_from([
            "cordy",
            "agent",
            "create",
            "--name",
            "Builder",
            "--runtime-id",
            "runtime-1",
            "--custom-env",
            &format!(r#"{{"TOKEN":"{secret}""#),
        ])
        .expect("invalid secret CLI");
        let Command::Agent(AgentArgs {
            command: AgentCommand::Create(args),
        }) = &invalid.command
        else {
            panic!("expected agent create");
        };
        let error = resolve_agent_secret_json(
            args.custom_env.as_deref(),
            args.custom_env_stdin,
            args.custom_env_file.as_deref(),
            "custom-env",
            false,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .expect_err("invalid custom env");
        assert!(error.to_string().contains("valid JSON object"));
        assert!(!error.to_string().contains(secret));

        let ambiguous = Cli::try_parse_from([
            "cordy",
            "agent",
            "create",
            "--name",
            "Builder",
            "--runtime-id",
            "runtime-1",
            "--mcp-config",
            "{}",
            "--mcp-config-stdin",
        ])
        .expect("ambiguous MCP CLI");
        let Command::Agent(AgentArgs {
            command: AgentCommand::Create(args),
        }) = &ambiguous.command
        else {
            panic!("expected agent create");
        };
        assert!(resolve_agent_secret_json(
            args.mcp_config.as_deref(),
            args.mcp_config_stdin,
            args.mcp_config_file.as_deref(),
            "mcp-config",
            true,
            &environment,
            &mut Cursor::new(b"{}".to_vec()),
        )
        .expect_err("ambiguous MCP inputs")
        .to_string()
        .contains("mutually exclusive"));
    }

    #[test]
    fn agent_create_validates_required_fields_and_concurrency() {
        let missing_name =
            Cli::try_parse_from(["cordy", "agent", "create", "--runtime-id", "runtime-1"])
                .expect("missing name parses for Go-compatible runtime validation");
        let Command::Agent(AgentArgs {
            command: AgentCommand::Create(args),
        }) = &missing_name.command
        else {
            panic!("expected agent create");
        };
        assert!(args.name.is_none());

        let invalid = Cli::try_parse_from([
            "cordy",
            "agent",
            "create",
            "--name",
            "Builder",
            "--runtime-id",
            "runtime-1",
            "--max-concurrent-tasks",
            "51",
        ])
        .expect("invalid concurrency parses for runtime validation");
        let Command::Agent(AgentArgs {
            command: AgentCommand::Create(args),
        }) = &invalid.command
        else {
            panic!("expected agent create");
        };
        assert_eq!(args.max_concurrent_tasks, Some(51));
    }

    #[tokio::test]
    async fn agent_update_puts_only_changed_fields_and_supports_mcp_clear() {
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/agents/agent-1",
            put(move |Json(body): Json<Value>| {
                let captured = Arc::clone(&captured_handler);
                async move {
                    *captured.lock().expect("captured body") = Some(body);
                    Json(serde_json::json!({"id":"agent-1","name":"Builder v2"}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "agent",
            "update",
            "agent-1",
            "--name",
            "Builder v2",
            "--thinking-level",
            "",
            "--mcp-config",
            "null",
            "--permission-mode",
            "private",
            "--output",
            "table",
        ])
        .expect("agent update CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update agent");
        assert_eq!(output.stdout, "Agent updated: Builder v2 (agent-1)\n");
        assert_eq!(
            captured
                .lock()
                .expect("captured body")
                .clone()
                .expect("body"),
            serde_json::json!({
                "name":"Builder v2",
                "thinking_level":"",
                "mcp_config":null,
                "permission_mode":"private",
                "invocation_targets":[]
            })
        );
        server.abort();
    }

    #[tokio::test]
    async fn agent_update_rejects_no_changes_and_does_not_expose_custom_env() {
        assert!(
            Cli::try_parse_from(["cordy", "agent", "update", "agent-1", "--custom-env", "{}"])
                .is_err()
        );
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["cordy", "agent", "update", "agent-1"]).expect("agent update CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("no changes");
        assert!(error.to_string().contains("no fields to update"));
        assert!(error.to_string().contains("cordy agent env set <id>"));
    }

    #[tokio::test]
    async fn agent_lifecycle_and_tasks_match_go_requests_and_outputs() {
        let app = Router::new()
            .route(
                "/api/agents/agent-1/archive",
                post(|Json(body): Json<Value>| async move {
                    assert!(body.is_null());
                    Json(serde_json::json!({"id":"agent-1","name":"Builder","archived_at":"now"}))
                }),
            )
            .route(
                "/api/agents/agent-1/restore",
                post(|Json(body): Json<Value>| async move {
                    assert!(body.is_null());
                    Json(serde_json::json!({"id":"agent-1","name":"Builder","archived_at":null}))
                }),
            )
            .route(
                "/api/agents/agent-1/tasks",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "id":"task-1",
                        "issue_id":"issue-1",
                        "status":"completed",
                        "created_at":"2026-08-24T00:00:00Z",
                        "server_only":"preserved"
                    })])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        for (command, expected) in [
            ("archive", "Agent archived: Builder (agent-1)\n"),
            ("restore", "Agent restored: Builder (agent-1)\n"),
        ] {
            let cli =
                Cli::try_parse_from(["cordy", "agent", command, "agent-1", "--output", "table"])
                    .expect("agent lifecycle CLI");
            let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("agent lifecycle request");
            assert_eq!(output.stdout, expected);
        }

        let tasks =
            Cli::try_parse_from(["cordy", "agent", "tasks", "agent-1"]).expect("agent tasks CLI");
        let table = run_with_input(&tasks, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list agent tasks");
        assert!(table.stdout.starts_with("ID"));
        assert!(table.stdout.contains("task-1"));
        assert!(table.stdout.contains("issue-1"));
        assert!(table.stdout.contains("completed"));

        let tasks_json =
            Cli::try_parse_from(["cordy", "agent", "tasks", "agent-1", "--output", "json"])
                .expect("agent tasks JSON CLI");
        let json = run_with_input(
            &tasks_json,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect("list agent tasks JSON");
        assert_eq!(
            serde_json::from_str::<Value>(&json.stdout).expect("JSON")[0]["server_only"],
            "preserved"
        );
        server.abort();
    }

    #[tokio::test]
    async fn agent_avatar_prechecks_uploads_and_updates_agent() {
        let app = Router::new()
            .route(
                "/api/agents/agent-1",
                get(|| async { Json(serde_json::json!({"id":"agent-1","name":"Builder"})) }).put(
                    |Json(body): Json<Value>| async move {
                        assert_eq!(body["avatar_url"], "https://cdn.example/avatar.png");
                        Json(serde_json::json!({"id":"agent-1","avatar_url":body["avatar_url"]}))
                    },
                ),
            )
            .route(
                "/api/upload-file",
                post(|request: Request| async move {
                    assert!(request
                        .headers()
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")));
                    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("multipart body");
                    let body = String::from_utf8_lossy(&bytes);
                    assert!(body.contains("filename=\"avatar.PNG\""));
                    assert!(body.contains("fake-png-data"));
                    Json(serde_json::json!({
                        "id":"attachment-1",
                        "url":"https://cdn.example/avatar.png"
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("avatar.PNG"), b"fake-png-data").expect("avatar");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "agent",
            "avatar",
            "agent-1",
            "--file",
            "avatar.PNG",
            "--output",
            "table",
        ])
        .expect("agent avatar CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("upload avatar");
        assert!(output.stdout.starts_with("ID"));
        assert!(output.stdout.contains("attachment-1"));
        assert!(output.stdout.contains("agent-1"));
        assert!(output.stdout.contains("https://cdn.example/avatar.png"));
        server.abort();
    }

    #[tokio::test]
    async fn agent_avatar_rejects_missing_bad_and_oversized_files_before_api_calls() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("avatar.txt"), b"not an image").expect("bad avatar");
        fs::write(cwd.path().join("large.png"), vec![0; (5 << 20) + 1]).expect("large avatar");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_TOKEN", "token-1");
        for (args, message) in [
            (
                vec!["cordy", "agent", "avatar", "agent-1"],
                "--file is required",
            ),
            (
                vec![
                    "cordy",
                    "agent",
                    "avatar",
                    "agent-1",
                    "--file",
                    "avatar.txt",
                ],
                "unsupported file format",
            ),
            (
                vec!["cordy", "agent", "avatar", "agent-1", "--file", "large.png"],
                "file too large",
            ),
        ] {
            let cli = Cli::try_parse_from(args).expect("agent avatar CLI");
            let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect_err("avatar validation");
            assert!(error.to_string().contains(message), "{error:#}");
        }
    }

    #[tokio::test]
    async fn agent_skills_list_set_and_add_match_go_contract() {
        let app = Router::new()
            .route(
                "/api/agents/agent-1/skills",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "id":"skill-1","name":"Review","description":"Reviews code"
                    })])
                })
                .put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"skill_ids":[]}));
                    Json(Vec::<Value>::new())
                }),
            )
            .route(
                "/api/agents/agent-1/skills/add",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"skill_ids":["skill-1","skill-2"]}));
                    Json(vec![serde_json::json!({
                        "id":"skill-1","name":"Review","description":"Reviews code",
                        "server_only":"preserved"
                    })])
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        let list = Cli::try_parse_from(["cordy", "agent", "skills", "list", "agent-1"])
            .expect("agent skills list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list skills");
        assert!(listed.stdout.starts_with("ID"));
        assert!(listed.stdout.contains("Reviews code"));

        let set = Cli::try_parse_from([
            "cordy",
            "agent",
            "skills",
            "set",
            "agent-1",
            "--skill-ids",
            "",
            "--output",
            "table",
        ])
        .expect("agent skills clear CLI");
        let cleared = run_with_input(&set, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("clear skills");
        assert_eq!(cleared.stdout, "No skills assigned to agent agent-1\n");

        let add = Cli::try_parse_from([
            "cordy",
            "agent",
            "skills",
            "add",
            "agent-1",
            "--skill-ids",
            " skill-1,skill-2 ",
        ])
        .expect("agent skills add CLI");
        let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add skills");
        assert_eq!(
            serde_json::from_str::<Value>(&added.stdout).expect("JSON")[0]["server_only"],
            "preserved"
        );
        server.abort();
    }

    #[tokio::test]
    async fn agent_skills_mutations_enforce_go_skill_id_requirements() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_TOKEN", "token-1");
        for (command, skill_ids, expected) in [
            ("set", None, "--skill-ids is required"),
            ("add", None, "--skill-ids is required"),
            (
                "add",
                Some(" , "),
                "--skill-ids must include at least one skill ID",
            ),
        ] {
            let mut argv = vec!["cordy", "agent", "skills", command, "agent-1"];
            if let Some(skill_ids) = skill_ids {
                argv.extend(["--skill-ids", skill_ids]);
            }
            let cli = Cli::try_parse_from(argv).expect("agent skills mutation CLI");
            let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect_err("skill IDs required");
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[tokio::test]
    async fn agent_env_get_and_set_use_audited_endpoint_and_preserve_values() {
        let app = Router::new().route(
            "/api/agents/agent-1/env",
            get(|| async {
                Json(serde_json::json!({
                    "custom_env":{"API_KEY":"plaintext","COUNT":"2"}
                }))
            })
            .put(|Json(body): Json<Value>| async move {
                assert_eq!(
                    body,
                    serde_json::json!({"custom_env":{"API_KEY":"****","NEW":"value"}})
                );
                Json(serde_json::json!({
                    "custom_env":{"API_KEY":"plaintext","NEW":"value"}
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        let get = Cli::try_parse_from([
            "cordy", "agent", "env", "get", "agent-1", "--output", "table",
        ])
        .expect("agent env get CLI");
        let env = run_with_input(&get, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get agent env");
        assert!(env.stdout.starts_with("KEY"));
        assert!(env.stdout.contains("API_KEY"));
        assert!(env.stdout.contains("plaintext"));

        let set = Cli::try_parse_from([
            "cordy",
            "agent",
            "env",
            "set",
            "agent-1",
            "--custom-env-stdin",
            "--output",
            "table",
        ])
        .expect("agent env set CLI");
        let updated = run_with_input(
            &set,
            &environment,
            &mut Cursor::new(br#"{"API_KEY":"****","NEW":"value"}"#.to_vec()),
        )
        .await
        .expect("set agent env");
        assert_eq!(updated.stdout, "Env updated for agent agent-1 (2 keys)\n");
        server.abort();
    }

    #[tokio::test]
    async fn agent_env_set_requires_one_secret_safe_input_channel() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "agent", "env", "set", "agent-1"])
            .expect("agent env set CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("env input required");
        assert!(error
            .to_string()
            .contains("specify the new env via --custom-env"));
    }

    #[test]
    fn agent_mcp_paths_trim_and_escape_each_identifier() {
        assert_eq!(
            agent_mcp_path(" agent/one ", &["server/two", "enabled"]),
            "/api/agents/agent%2Fone/mcp-servers/server%2Ftwo/enabled"
        );
    }

    #[tokio::test]
    async fn agent_mcp_commands_match_go_api_and_redacted_output_contract() {
        let server_value = || {
            serde_json::json!({
                "id":"server-1","name":"linear","transport":"http","enabled":true,
                "config":{"headers":{"Authorization":"secret"}}
            })
        };
        let app = Router::new()
            .route(
                "/api/agents/agent-1/mcp-servers",
                get({
                    let value = server_value();
                    move || {
                        let value = value.clone();
                        async move { Json(vec![value]) }
                    }
                })
                .post({
                    let value = server_value();
                    move |Json(body): Json<Value>| {
                        let value = value.clone();
                        async move {
                            assert_eq!(body, serde_json::json!({"server_id":"server-1"}));
                            Json(vec![value])
                        }
                    }
                }),
            )
            .route(
                "/api/agents/agent-1/mcp-servers/server-1/enabled",
                put({
                    let value = server_value();
                    move |Json(body): Json<Value>| {
                        let value = value.clone();
                        async move {
                            assert!(body.get("enabled").and_then(Value::as_bool).is_some());
                            Json(vec![value])
                        }
                    }
                }),
            )
            .route(
                "/api/agents/agent-1/mcp-servers/server-1",
                delete_route(|| async { Json(Vec::<Value>::new()) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        for argv in [
            vec!["cordy", "agent", "mcp", "list", "agent-1"],
            vec!["cordy", "agent", "mcp", "add", "agent-1", "server-1"],
            vec!["cordy", "agent", "mcp", "enable", "agent-1", "server-1"],
            vec!["cordy", "agent", "mcp", "disable", "agent-1", "server-1"],
        ] {
            let cli = Cli::try_parse_from(argv).expect("agent MCP CLI");
            let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("agent MCP request");
            assert!(output.stdout.contains("linear"));
            assert!(output.stdout.contains("enabled"));
            assert!(!output.stdout.contains("secret"));
            assert!(!output.stdout.contains("Authorization"));
        }

        let remove =
            Cli::try_parse_from(["cordy", "agent", "mcp", "remove", "agent-1", "server-1"])
                .expect("agent MCP remove CLI");
        let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("remove agent MCP server");
        assert_eq!(removed.stdout, "no MCP servers\n");
        server.abort();
    }

    #[tokio::test]
    async fn agent_copy_copies_only_portable_same_runtime_fields() {
        let source = serde_json::json!({
            "id":"agent-source","name":"Source","runtime_id":"runtime-1",
            "description":"description","instructions":"instructions",
            "avatar_url":"https://cdn.example/avatar.png",
            "custom_args":["--foo"],"max_concurrent_tasks":9,
            "model":"model-1","thinking_level":"high","service_tier":"priority",
            "permission_mode":"public_to",
            "invocation_targets":[{"target_type":"workspace"}],
            "skills":[{"id":"skill-1"},{"id":"skill-2"}],
            "has_custom_env":true,"custom_env_key_count":2,"mcp_config_redacted":true,
            "runtime_config":{"machine":"must-not-copy"}
        });
        let captured = Arc::new(Mutex::new(None));
        let captured_handler = Arc::clone(&captured);
        let app = Router::new()
            .route(
                "/api/agents/agent-source",
                get(move || {
                    let source = source.clone();
                    async move { Json(source) }
                }),
            )
            .route(
                "/api/agents",
                post(move |Json(body): Json<Value>| {
                    let captured = Arc::clone(&captured_handler);
                    async move {
                        *captured.lock().expect("captured body") = Some(body);
                        Json(serde_json::json!({"id":"agent-copy","name":"Source (copy)"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from(["cordy", "agent", "copy", "agent-source"])
            .expect("agent copy CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("copy agent");
        assert_eq!(
            serde_json::from_str::<Value>(&output.stdout).expect("JSON")["id"],
            "agent-copy"
        );
        let body = captured
            .lock()
            .expect("captured body")
            .clone()
            .expect("body");
        assert_eq!(body["name"], "Source (copy)");
        assert_eq!(body["runtime_id"], "runtime-1");
        assert_eq!(body["description"], "description");
        assert_eq!(body["instructions"], "instructions");
        assert_eq!(body["avatar_url"], "https://cdn.example/avatar.png");
        assert_eq!(body["custom_args"], serde_json::json!(["--foo"]));
        assert_eq!(body["max_concurrent_tasks"], 9);
        assert_eq!(body["model"], "model-1");
        assert_eq!(body["thinking_level"], "high");
        assert_eq!(body["service_tier"], "priority");
        assert_eq!(body["permission_mode"], "public_to");
        assert_eq!(body["skill_ids"], serde_json::json!(["skill-1", "skill-2"]));
        for forbidden in [
            "custom_env",
            "mcp_config",
            "runtime_config",
            "has_custom_env",
            "custom_env_key_count",
            "mcp_config_redacted",
        ] {
            assert!(body.get(forbidden).is_none(), "copied {forbidden}");
        }
        server.abort();
    }

    #[tokio::test]
    async fn agent_copy_cross_runtime_requires_model_and_drops_runtime_fields() {
        let posts = Arc::new(Mutex::new(Vec::<Value>::new()));
        let posts_handler = Arc::clone(&posts);
        let source = serde_json::json!({
            "id":"agent-source","name":"Source","runtime_id":"runtime-1",
            "model":"old-model","thinking_level":"high","service_tier":"priority",
            "max_concurrent_tasks":0
        });
        let app = Router::new()
            .route(
                "/api/agents/agent-source",
                get(move || {
                    let source = source.clone();
                    async move { Json(source) }
                }),
            )
            .route(
                "/api/agents",
                post(move |Json(body): Json<Value>| {
                    let posts = Arc::clone(&posts_handler);
                    async move {
                        posts.lock().expect("posts").push(body);
                        Json(serde_json::json!({"id":"agent-copy","name":"Source (copy)"}))
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        let missing_model = Cli::try_parse_from([
            "cordy",
            "agent",
            "copy",
            "agent-source",
            "--runtime-id",
            "runtime-2",
        ])
        .expect("cross-runtime copy CLI");
        let error = run_with_input(
            &missing_model,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect_err("model required");
        assert!(error.to_string().contains("requires --model"));
        assert!(posts.lock().expect("posts").is_empty());

        let copy = Cli::try_parse_from([
            "cordy",
            "agent",
            "copy",
            "agent-source",
            "--runtime-id",
            "runtime-2",
            "--model",
            "",
            "--no-skills",
        ])
        .expect("cross-runtime copy with model CLI");
        run_with_input(&copy, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("copy across runtime");
        let body = posts.lock().expect("posts")[0].clone();
        assert_eq!(body["runtime_id"], "runtime-2");
        assert_eq!(body["model"], "");
        assert!(body.get("thinking_level").is_none());
        assert!(body.get("service_tier").is_none());
        assert!(body.get("max_concurrent_tasks").is_none());
        assert!(body.get("skill_ids").is_none());
        server.abort();
    }

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

    #[tokio::test]
    async fn runtime_read_commands_match_go_requests_and_tables() {
        let app = Router::new()
            .route(
                "/api/runtimes",
                get(|| async {
                    Json(vec![serde_json::json!({
                        "id":"runtime-1","name":"Mac","runtime_mode":"local",
                        "provider":"codex","status":"online","last_seen_at":"now",
                        "server_only":"preserved"
                    })])
                }),
            )
            .route(
                "/api/runtimes/runtime-1/usage",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("days=30"));
                    Json(vec![serde_json::json!({
                        "date":"2026-08-24","provider":"codex","model":"gpt",
                        "input_tokens":10,"output_tokens":5,
                        "cache_read_tokens":2,"cache_write_tokens":1
                    })])
                }),
            )
            .route(
                "/api/runtimes/runtime-1/activity",
                get(|| async { Json(vec![serde_json::json!({"hour":"12","count":3})]) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        let list = Cli::try_parse_from(["cordy", "runtime", "list"]).expect("runtime list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list runtimes");
        assert!(listed.stdout.starts_with("ID"));
        assert!(listed.stdout.contains("runtime-1"));
        assert!(listed.stdout.contains("codex"));

        let usage = Cli::try_parse_from(["cordy", "runtime", "usage", "runtime-1", "--days", "30"])
            .expect("runtime usage CLI");
        let used = run_with_input(&usage, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("runtime usage");
        assert!(used.stdout.starts_with("DATE"));
        assert!(used.stdout.contains("2026-08-24"));
        assert!(used.stdout.contains("10"));

        let activity = Cli::try_parse_from([
            "cordy",
            "runtime",
            "activity",
            "runtime-1",
            "--output",
            "json",
        ])
        .expect("runtime activity CLI");
        let active = run_with_input(&activity, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("runtime activity");
        assert_eq!(
            serde_json::from_str::<Value>(&active.stdout).expect("JSON")[0]["count"],
            3
        );
        server.abort();
    }

    #[tokio::test]
    async fn runtime_usage_rejects_days_outside_go_range() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", "http://127.0.0.1:9");
        environment.set("CORDY_TOKEN", "token-1");
        for days in ["0", "366"] {
            let cli =
                Cli::try_parse_from(["cordy", "runtime", "usage", "runtime-1", "--days", days])
                    .expect("runtime usage CLI");
            let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect_err("days range");
            assert_eq!(error.to_string(), "--days must be between 1 and 365");
        }
    }

    #[tokio::test]
    async fn runtime_rename_and_cascade_delete_match_go_contract() {
        let app = Router::new()
            .route(
                "/api/runtimes/runtime-1",
                patch(|Json(body): Json<Value>| async move {
                    assert_eq!(
                        body,
                        serde_json::json!({"custom_name":"Build Mac","apply_to_machine":true})
                    );
                    Json(serde_json::json!({
                        "id":"runtime-1","name":"Build Mac","custom_name":"Build Mac"
                    }))
                })
                .delete(|| async {
                    (
                        axum::http::StatusCode::CONFLICT,
                        Json(serde_json::json!({
                            "code":"runtime_has_active_agents",
                            "error":"runtime has active agents",
                            "active_agents":[
                                {"id":"agent-1","name":"Builder"},
                                {"id":"agent-2","name":""}
                            ]
                        })),
                    )
                }),
            )
            .route(
                "/api/runtimes/runtime-1/unbind-agents-and-delete",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(
                        body,
                        serde_json::json!({"expected_active_agent_ids":["agent-1","agent-2"]})
                    );
                    Json(serde_json::json!({
                        "agents_unbound":2,"autopilots_paused":1
                    }))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");

        let rename = Cli::try_parse_from([
            "cordy",
            "runtime",
            "rename",
            "runtime-1",
            "Build Mac",
            "--machine",
        ])
        .expect("runtime rename CLI");
        let renamed = run_with_input(&rename, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("rename runtime");
        assert!(renamed.stdout.is_empty());
        assert_eq!(renamed.stderr, "Runtime renamed to \"Build Mac\".\n");

        let delete = Cli::try_parse_from(["cordy", "runtime", "delete", "runtime-1"])
            .expect("runtime delete CLI");
        let conflict = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("active agents conflict");
        assert!(conflict.to_string().contains("Builder (agent-1), agent-2"));
        assert!(conflict.to_string().contains("--cascade"));

        let cascade = Cli::try_parse_from(["cordy", "runtime", "delete", "runtime-1", "--cascade"])
            .expect("runtime cascade delete CLI");
        let deleted = run_with_input(&cascade, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("cascade delete runtime");
        assert!(deleted.stdout.is_empty());
        assert_eq!(
            deleted.stderr,
            "Runtime runtime-1 deleted; unbound 2 agent(s) and paused 1 autopilot(s).\n"
        );
        server.abort();
    }

    #[tokio::test]
    async fn runtime_delete_strict_success_returns_go_json_mirror() {
        let app = Router::new().route(
            "/api/runtimes/runtime-1",
            delete_route(|| async { axum::http::StatusCode::NO_CONTENT }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "runtime",
            "delete",
            "runtime-1",
            "--output",
            "json",
        ])
        .expect("runtime delete JSON CLI");
        let deleted = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete runtime");
        assert_eq!(
            serde_json::from_str::<Value>(&deleted.stdout).expect("JSON"),
            serde_json::json!({"id":"runtime-1","deleted":true})
        );
        server.abort();
    }

    #[tokio::test]
    async fn runtime_update_initiates_and_waits_with_injected_poll_policy() {
        let polls = Arc::new(Mutex::new(0usize));
        let polls_handler = Arc::clone(&polls);
        let app = Router::new()
            .route(
                "/api/runtimes/runtime-1/update",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"target_version":"v2.0.0"}));
                    Json(serde_json::json!({"id":"update-1","status":"pending"}))
                }),
            )
            .route(
                "/api/runtimes/runtime-1/update/update-1",
                get(move || {
                    let polls = Arc::clone(&polls_handler);
                    async move {
                        let mut count = polls.lock().expect("poll count");
                        *count += 1;
                        if *count == 1 {
                            Json(serde_json::json!({"id":"update-1","status":"running"}))
                        } else {
                            Json(serde_json::json!({
                                "id":"update-1","status":"completed","output":"updated"
                            }))
                        }
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "runtime",
            "update",
            "runtime-1",
            "--target-version",
            "v2.0.0",
            "--wait",
            "--output",
            "table",
        ])
        .expect("runtime update CLI");
        let Command::Runtime(RuntimeArgs {
            command:
                RuntimeCommand::Update {
                    runtime_id,
                    target_version,
                    output,
                    wait,
                },
        }) = &cli.command
        else {
            panic!("expected runtime update");
        };
        let updated = run_runtime_update_with_policy(
            &cli,
            &environment,
            runtime_id,
            target_version.as_deref(),
            *output,
            *wait,
            Duration::from_millis(1),
            Duration::from_secs(1),
        )
        .await
        .expect("wait for runtime update");
        assert_eq!(updated.stdout, "Update completed: updated\n");
        assert_eq!(*polls.lock().expect("poll count"), 2);
        server.abort();
    }

    #[tokio::test]
    async fn runtime_update_timeout_reports_last_status() {
        let app = Router::new()
            .route(
                "/api/runtimes/runtime-1/update",
                post(|| async { Json(serde_json::json!({"id":"update-1","status":"pending"})) }),
            )
            .route(
                "/api/runtimes/runtime-1/update/update-1",
                get(|| async { Json(serde_json::json!({"id":"update-1","status":"running"})) }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "runtime",
            "update",
            "runtime-1",
            "--target-version",
            "v2",
            "--wait",
        ])
        .expect("runtime update CLI");
        let error = run_runtime_update_with_policy(
            &cli,
            &environment,
            "runtime-1",
            Some("v2"),
            OutputFormat::Json,
            true,
            Duration::from_millis(1),
            Duration::from_millis(10),
        )
        .await
        .expect_err("runtime update timeout");
        assert!(error
            .to_string()
            .starts_with("timed out waiting for update (last status:"));
        server.abort();
    }

    #[test]
    fn runtime_update_terminal_table_outputs_match_go() {
        assert_eq!(
            format_runtime_update_result(
                &serde_json::json!({"status":"failed","error":"boom"}),
                OutputFormat::Table,
                true,
            )
            .expect("failed update output")
            .stdout,
            "Update failed: boom\n"
        );
        assert_eq!(
            format_runtime_update_result(
                &serde_json::json!({"status":"timeout","error":"daemon timeout"}),
                OutputFormat::Table,
                true,
            )
            .expect("timeout update output")
            .stdout,
            "Update timeout: daemon timeout\n"
        );
    }

    #[tokio::test]
    async fn runtime_profile_registry_commands_match_go_contract() {
        let collection = "/api/workspaces/workspace-1/runtime-profiles";
        let resource = "/api/workspaces/workspace-1/runtime-profiles/profile-1";
        let app = Router::new()
            .route(
                collection,
                get(|| async {
                    Json(serde_json::json!({"runtime_profiles":[
                        {"id":"profile-2","display_name":"Zulu","protocol_family":"codex","command_name":"z","enabled":true},
                        {"id":"profile-1","display_name":"Alpha","protocol_family":"claude","command_name":"a","enabled":false}
                    ]}))
                })
                .post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["protocol_family"], "codex");
                    assert_eq!(body["command_name"], "wrapper");
                    assert_eq!(body["display_name"], "Wrapper");
                    assert!(body.get("description").is_none());
                    Json(serde_json::json!({
                        "id":"profile-1","display_name":"Wrapper","protocol_family":"codex",
                        "command_name":"wrapper","enabled":true,"server_only":"preserved"
                    }))
                }),
            )
            .route(
                resource,
                patch(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"description":"","enabled":false}));
                    Json(serde_json::json!({
                        "id":"profile-1","display_name":"Wrapper","protocol_family":"codex",
                        "command_name":"wrapper","description":"","enabled":false
                    }))
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let list = Cli::try_parse_from(["cordy", "runtime", "profile", "list"])
            .expect("runtime profile list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list runtime profiles");
        assert!(
            listed.stdout.find("Alpha").expect("Alpha") < listed.stdout.find("Zulu").expect("Zulu")
        );

        let create = Cli::try_parse_from([
            "cordy",
            "runtime",
            "profile",
            "create",
            "--protocol-family",
            "codex",
            "--command-name",
            "wrapper",
            "--display-name",
            "Wrapper",
        ])
        .expect("runtime profile create CLI");
        let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create runtime profile");
        assert_eq!(
            serde_json::from_str::<Value>(&created.stdout).expect("JSON")["server_only"],
            "preserved"
        );

        let update = Cli::try_parse_from([
            "cordy",
            "runtime",
            "profile",
            "update",
            "profile-1",
            "--description",
            "",
            "--enabled=false",
        ])
        .expect("runtime profile update CLI");
        run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update runtime profile");

        let delete = Cli::try_parse_from(["cordy", "runtime", "profile", "delete", "profile-1"])
            .expect("runtime profile delete CLI");
        let deleted = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("delete runtime profile");
        assert_eq!(deleted.stdout, "Deleted runtime profile profile-1\n");
        server.abort();
    }

    #[tokio::test]
    async fn runtime_profile_validates_create_update_and_delete_conflict() {
        let app = Router::new().route(
            "/api/workspaces/workspace-1/runtime-profiles/profile-1",
            delete_route(|| async {
                (
                    axum::http::StatusCode::CONFLICT,
                    "active agents remain bound",
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let invalid = Cli::try_parse_from([
            "cordy",
            "runtime",
            "profile",
            "create",
            "--protocol-family",
            "unknown",
            "--command-name",
            "wrapper",
            "--display-name",
            "Wrapper",
        ])
        .expect("invalid family parses for runtime validation");
        let error = run_with_input(&invalid, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("invalid protocol family");
        assert!(error.to_string().contains("must be one of"));

        let empty_update =
            Cli::try_parse_from(["cordy", "runtime", "profile", "update", "profile-1"])
                .expect("empty runtime profile update CLI");
        let error = run_with_input(
            &empty_update,
            &environment,
            &mut Cursor::new(Vec::<u8>::new()),
        )
        .await
        .expect_err("no fields");
        assert!(error.to_string().contains("no fields to update"));

        let delete = Cli::try_parse_from(["cordy", "runtime", "profile", "delete", "profile-1"])
            .expect("runtime profile delete CLI");
        let error = run_with_input(&delete, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("profile conflict");
        assert_eq!(
            error.to_string(),
            "cannot delete runtime profile profile-1: active agents remain bound"
        );
        server.abort();
    }

    #[tokio::test]
    async fn runtime_profile_path_overrides_are_locked_atomic_and_profile_local() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            r#"{"server_url":"https://api.example","unknown":{"keep":true}}"#,
        )
        .expect("profile config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let set = Cli::try_parse_from([
            "cordy",
            "--profile",
            "dev",
            "runtime",
            "profile",
            "set-path",
            "profile-1",
            "--path",
            "/opt/bin/company-codex",
        ])
        .expect("runtime profile set-path CLI");
        let output = run_with_input(&set, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("set profile path");
        assert!(output.stdout.contains("Pinned runtime profile profile-1"));
        let document: Value = serde_json::from_slice(
            &fs::read(profile_dir.join("config.json")).expect("updated config"),
        )
        .expect("config JSON");
        assert_eq!(document["server_url"], "https://api.example");
        assert_eq!(document["unknown"]["keep"], true);
        assert_eq!(
            document["profile_command_overrides"]["profile-1"],
            "/opt/bin/company-codex"
        );
        assert!(!home.path().join(".cordy/config.json").exists());

        let unset = Cli::try_parse_from([
            "cordy",
            "--profile",
            "dev",
            "runtime",
            "profile",
            "unset-path",
            "profile-1",
        ])
        .expect("runtime profile unset-path CLI");
        let output = run_with_input(&unset, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("unset profile path");
        assert!(output.stdout.contains("Removed per-machine path override"));
        let document: Value = serde_json::from_slice(
            &fs::read(profile_dir.join("config.json")).expect("updated config"),
        )
        .expect("config JSON");
        assert!(document.get("profile_command_overrides").is_none());
        assert_eq!(document["unknown"]["keep"], true);

        let output = run_with_input(&unset, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("idempotent unset");
        assert_eq!(
            output.stdout,
            "No per-machine path override set for runtime profile profile-1.\n"
        );
    }

    #[tokio::test]
    async fn runtime_profile_path_mutation_fails_closed_in_task_context() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let config_dir = home.path().join(".cordy");
        fs::create_dir_all(&config_dir).expect("config dir");
        let owner = br#"{"profile_command_overrides":{"owner":"/owner/bin"},"token":"mul_owner"}"#;
        fs::write(config_dir.join("config.json"), owner).expect("owner config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_AGENT_ID", "agent-1");
        environment.set("CORDY_TASK_ID", "task-1");
        let cli = Cli::try_parse_from([
            "cordy",
            "runtime",
            "profile",
            "set-path",
            "profile-1",
            "--path",
            "/opt/bin/runtime",
        ])
        .expect("runtime profile set-path CLI");
        let error = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("task context denied");
        assert!(error
            .to_string()
            .contains("not available inside a daemon-managed task"));
        assert_eq!(
            fs::read(config_dir.join("config.json")).expect("owner config"),
            owner
        );
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
    fn project_read_parser_and_tables_match_go_registry_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "list",
            "--status",
            "in_progress",
            "--full-id",
            "--output",
            "json",
        ])
        .expect("project list CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::List {
                    output,
                    full_id,
                    status,
                },
        }) = &cli.command
        else {
            panic!("expected project list");
        };
        assert_eq!(*output, OutputFormat::Json);
        assert!(*full_id);
        assert_eq!(status.as_deref(), Some("in_progress"));

        let project = serde_json::json!({
            "id":"11111111-1111-1111-1111-111111111111","title":"Migration",
            "status":"in_progress","lead_type":"member","lead_id":"member-1",
            "created_at":"2026-08-24T12:34:56Z","description":"Rust port"
        });
        let actors = IssueActorNames(HashMap::from([("member:member-1".into(), "Ada".into())]));
        let list = format_project_list_table(std::slice::from_ref(&project), &actors, false);
        assert!(list.starts_with("ID"));
        assert!(list.contains("11111111"));
        assert!(list.contains("Migration"));
        assert!(list.contains("member:Ada"));
        assert!(list.contains("2026-08-24"));
        let details = format_project_details_table(&project, &actors);
        assert!(details.contains("11111111-1111-1111-1111-111111111111"));
        assert!(details.contains("Rust port"));
    }

    #[tokio::test]
    async fn project_list_sends_workspace_status_and_preserves_json_array() {
        let app = Router::new().route(
            "/api/projects",
            get(|request: Request| async move {
                let query = request.uri().query().unwrap_or_default();
                assert!(query.contains("workspace_id=workspace-1"));
                assert!(query.contains("status=in_progress"));
                Json(serde_json::json!({
                    "projects":[{"id":"project-1","title":"Migration","status":"in_progress"}]
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
            "project",
            "list",
            "--status",
            "in_progress",
            "--output",
            "json",
        ])
        .expect("project list CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list projects");
        let projects: Value = serde_json::from_str(&output.stdout).expect("projects JSON");
        assert_eq!(projects[0]["title"], "Migration");
        task.abort();
    }

    #[tokio::test]
    async fn project_get_resolves_prefix_and_reports_attached_resources() {
        let project_id = "abcd1234-0000-0000-0000-000000000000";
        let app = Router::new()
            .route(
                "/api/projects",
                get(move || async move {
                    Json(serde_json::json!({
                        "projects":[{"id":project_id,"title":"Migration","status":"planned"}]
                    }))
                }),
            )
            .route(
                "/api/projects/abcd1234-0000-0000-0000-000000000000",
                get(move || async move {
                    Json(serde_json::json!({
                        "id":project_id,"title":"Migration","status":"planned",
                        "description":"Rust port","resource_count":2
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
        let cli = Cli::try_parse_from(["cordy", "project", "get", "abcd", "--output", "table"])
            .expect("project get CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get project");
        assert!(output.stdout.contains("Migration"));
        assert!(output.stderr.contains("2 resource(s) attached"));
        assert!(output.stderr.contains(project_id));
        task.abort();
    }

    #[test]
    fn project_mutation_parser_and_status_validation_match_go_contract() {
        let create = Cli::try_parse_from([
            "cordy",
            "project",
            "create",
            "--title",
            "Migration",
            "--status",
            "planned",
            "--repo",
            "https://github.com/acme/one",
            "--repo",
            "https://github.com/acme/two",
        ])
        .expect("project create CLI");
        let Command::Project(ProjectArgs {
            command: ProjectCommand::Create(args),
        }) = &create.command
        else {
            panic!("expected project create");
        };
        assert_eq!(args.repo.len(), 2);
        for status in PROJECT_STATUSES {
            validate_project_status(status).expect("valid project status");
        }
        assert!(validate_project_status("active")
            .expect_err("invalid status")
            .to_string()
            .contains("planned"));

        let update = Cli::try_parse_from([
            "cordy",
            "project",
            "update",
            "11111111-1111-1111-1111-111111111111",
            "--start-date=",
            "--due-date=",
        ])
        .expect("project update clears");
        let Command::Project(ProjectArgs {
            command: ProjectCommand::Update(args),
        }) = &update.command
        else {
            panic!("expected project update");
        };
        assert_eq!(args.start_date.as_deref(), Some(""));
        assert_eq!(args.due_date.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn project_create_bundles_repos_and_status_updates_return_go_outputs() {
        let project_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new()
            .route(
                "/api/projects",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["title"], "Migration");
                    assert_eq!(body["status"], "planned");
                    assert_eq!(body["resources"].as_array().expect("resources").len(), 2);
                    assert_eq!(
                        body["resources"][0]["resource_ref"]["url"],
                        "https://github.com/acme/one"
                    );
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "title":"Migration","status":"planned"
                    }))
                }),
            )
            .route(
                "/api/projects/11111111-1111-1111-1111-111111111111",
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body, serde_json::json!({"status":"completed"}));
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "title":"Migration","status":"completed"
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
        let create = Cli::try_parse_from([
            "cordy",
            "project",
            "create",
            "--title",
            "Migration",
            "--status",
            "planned",
            "--repo",
            "https://github.com/acme/one",
            "--repo",
            "https://github.com/acme/two",
        ])
        .expect("project create CLI");
        let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create project");
        assert_eq!(
            serde_json::from_str::<Value>(&created.stdout).expect("project JSON")["id"],
            project_id
        );

        let status = Cli::try_parse_from([
            "cordy",
            "project",
            "status",
            project_id,
            "completed",
            "--output",
            "table",
        ])
        .expect("project status CLI");
        let updated = run_with_input(&status, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update project status");
        assert!(updated.stdout.is_empty());
        assert_eq!(
            updated.stderr,
            "Project Migration status changed to completed.\n"
        );
        task.abort();
    }

    #[test]
    fn project_resource_add_parser_and_ref_shortcuts_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "add",
            "11111111-1111-1111-1111-111111111111",
            "--url",
            "https://github.com/acme/cordy",
            "--ref",
            "2024",
            "--default-branch-hint",
            "main",
            "--label",
            "Cordy",
        ])
        .expect("project resource add CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        }) = &cli.command
        else {
            panic!("expected project resource add");
        };
        assert_eq!(args.resource_type, "github_repo");
        assert_eq!(
            build_project_resource_add_ref(args).expect("github ref"),
            serde_json::json!({
                "url":"https://github.com/acme/cordy",
                "ref":"2024",
                "default_branch_hint":"main"
            })
        );

        let generic = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "add",
            "11111111-1111-1111-1111-111111111111",
            "--type",
            "documentation",
            "--ref",
            r#"{"url":"https://docs.example.com"}"#,
        ])
        .expect("generic project resource CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Add(args),
                }),
        }) = &generic.command
        else {
            panic!("expected generic project resource add");
        };
        assert_eq!(
            build_project_resource_add_ref(args).expect("generic ref"),
            serde_json::json!({"url":"https://docs.example.com"})
        );
    }

    #[tokio::test]
    async fn project_resource_list_and_add_use_go_http_and_output_contracts() {
        let project_id = "11111111-1111-1111-1111-111111111111";
        let resource_id = "22222222-2222-2222-2222-222222222222";
        let app = Router::new().route(
            "/api/projects/11111111-1111-1111-1111-111111111111/resources",
            get(move || async move {
                Json(serde_json::json!({"resources":[{
                    "id":resource_id,"resource_type":"github_repo",
                    "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main"},
                    "label":"Cordy"
                }]}))
            })
            .post(|Json(body): Json<Value>| async move {
                assert_eq!(body["resource_type"], "local_directory");
                assert_eq!(body["resource_ref"]["local_path"], "/srv/cordy");
                assert_eq!(body["resource_ref"]["daemon_id"], "daemon-1");
                assert_eq!(body["resource_ref"]["execution_mode"], "worktree");
                Json(serde_json::json!({
                    "id":"33333333-3333-3333-3333-333333333333",
                    "resource_type":"local_directory",
                    "resource_ref":{"local_path":"/srv/cordy","daemon_id":"daemon-1","execution_mode":"worktree"}
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

        let list = Cli::try_parse_from([
            "cordy", "project", "resource", "list", project_id, "--output", "table",
        ])
        .expect("project resource list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list project resources");
        assert!(listed.stdout.contains("22222222"));
        assert!(listed
            .stdout
            .contains("https://github.com/acme/cordy @ main"));
        assert!(listed.stdout.contains("Cordy"));

        let add = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "add",
            project_id,
            "--type",
            "local_directory",
            "--local-path",
            "/srv/cordy",
            "--daemon-id",
            "daemon-1",
            "--execution-mode",
            "worktree",
            "--output",
            "table",
        ])
        .expect("project resource add CLI");
        let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add project resource");
        assert!(added
            .stdout
            .contains("33333333-3333-3333-3333-333333333333"));
        assert!(added.stdout.contains("/srv/cordy"));
        task.abort();
    }

    #[test]
    fn project_resource_update_rebuilds_opaque_refs_and_supports_clear_flags() {
        let cli = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "update",
            "11111111-1111-1111-1111-111111111111",
            "2222",
            "--default-branch-hint",
            "trunk",
            "--clear-label",
            "--position",
            "3",
            "--output",
            "table",
        ])
        .expect("project resource update CLI");
        let Command::Project(ProjectArgs {
            command:
                ProjectCommand::Resource(ProjectResourceArgs {
                    command: ProjectResourceCommand::Update(args),
                }),
        }) = &cli.command
        else {
            panic!("expected project resource update");
        };
        assert!(args.clear_label);
        assert_eq!(args.position, Some(3));
        let existing = serde_json::json!({
            "url":"https://github.com/acme/cordy",
            "ref":"main",
            "default_branch_hint":"main"
        });
        assert_eq!(
            build_project_resource_update_ref(args, "github_repo", existing.as_object())
                .expect("update ref")
                .expect("changed ref"),
            serde_json::json!({
                "url":"https://github.com/acme/cordy",
                "ref":"main",
                "default_branch_hint":"trunk"
            })
        );
    }

    #[tokio::test]
    async fn project_resource_update_and_remove_use_prefix_put_and_delete_contracts() {
        let project_id = "11111111-1111-1111-1111-111111111111";
        let resource_id = "22222222-2222-2222-2222-222222222222";
        let resource_path =
            "/api/projects/11111111-1111-1111-1111-111111111111/resources/22222222-2222-2222-2222-222222222222";
        let app = Router::new()
            .route(
                "/api/projects/11111111-1111-1111-1111-111111111111/resources",
                get(move || async move {
                    Json(serde_json::json!({"resources":[{
                        "id":resource_id,"resource_type":"github_repo",
                        "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main"},
                        "label":"Cordy"
                    }]}))
                }),
            )
            .route(
                resource_path,
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body["label"], Value::Null);
                    assert_eq!(body["position"], 3);
                    assert_eq!(
                        body["resource_ref"],
                        serde_json::json!({
                            "url":"https://github.com/acme/cordy",
                            "ref":"main",
                            "default_branch_hint":"trunk"
                        })
                    );
                    Json(serde_json::json!({
                        "id":"22222222-2222-2222-2222-222222222222",
                        "resource_type":"github_repo",
                        "resource_ref":{"url":"https://github.com/acme/cordy","ref":"main","default_branch_hint":"trunk"},
                        "label":""
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

        let update = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "update",
            project_id,
            "2222",
            "--default-branch-hint",
            "trunk",
            "--clear-label",
            "--position",
            "3",
            "--output",
            "table",
        ])
        .expect("project resource update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update project resource");
        assert!(updated.stdout.contains(resource_id));
        assert!(updated
            .stdout
            .contains("https://github.com/acme/cordy @ main"));

        let remove = Cli::try_parse_from([
            "cordy",
            "project",
            "resource",
            "remove",
            project_id,
            resource_id,
        ])
        .expect("project resource remove CLI");
        let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("remove project resource");
        assert!(removed.stdout.is_empty());
        assert_eq!(
            removed.stderr,
            format!("Resource {resource_id} removed from project {project_id}.\n")
        );
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

    #[tokio::test]
    async fn chat_history_and_thread_match_go_query_and_render_contracts() {
        let app = Router::new()
            .route(
                "/api/chat/history",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("before=cursor%2Fone&limit=25"));
                    Json(serde_json::json!({
                        "messages":[{
                            "ts":"2026-08-24T00:00:00Z","role":"user","author":"Ada",
                            "thread_id":"thread/1","reply_count":2,"text":"status?"
                        }],"next_cursor":"older"
                    }))
                }),
            )
            .route(
                "/api/chat/thread",
                get(|request: Request| async move {
                    assert_eq!(request.uri().query(), Some("id=thread%2F1"));
                    Json(serde_json::json!({"note":"thread is unavailable"}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TOKEN", "token-1");

        let history = Cli::try_parse_from([
            "cordy",
            "chat",
            "history",
            "--before",
            "cursor/one",
            "--limit",
            "25",
            "--output",
            "table",
        ])
        .expect("chat history CLI");
        let history = run_with_input(&history, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("chat history");
        assert!(history.stdout.starts_with("TS"));
        assert!(history.stdout.contains("thread/1"));
        assert!(history.stdout.contains("2"));
        assert!(history.stdout.contains("status?"));

        let thread =
            Cli::try_parse_from(["cordy", "chat", "thread", "thread/1", "--output", "table"])
                .expect("chat thread CLI");
        let thread = run_with_input(&thread, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("chat thread");
        assert_eq!(thread.stdout, "thread is unavailable\n");
        server.abort();
    }

    #[tokio::test]
    async fn attachment_upload_and_download_match_go_file_and_output_contracts() {
        let app = Router::new()
            .route(
                "/api/upload-file",
                post(|request: Request| async move {
                    let content_type = request
                        .headers()
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    assert!(content_type.starts_with("multipart/form-data; boundary="));
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("multipart body");
                    let body = String::from_utf8_lossy(&body);
                    assert!(body.contains("task-1"));
                    assert!(body.contains("chart[v2].png"));
                    Json(serde_json::json!({
                        "id":"attachment-1","content_type":"image/png",
                        "markdown_url":"/api/attachments/attachment-1/download"
                    }))
                }),
            )
            .route(
                "/api/attachments/attachment-1",
                get(|| async {
                    Json(serde_json::json!({
                        "id":"attachment-1","filename":"../report.txt",
                        "download_url":"/downloads/report.txt","size_bytes":15
                    }))
                }),
            )
            .route(
                "/downloads/report.txt",
                get(|request: Request| async move {
                    assert!(request.headers().contains_key("authorization"));
                    "attachment body"
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(cwd.path().join("chart[v2].png"), b"png bytes").expect("upload file");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "workspace-1");
        environment.set("CORDY_TASK_ID", "task-1");
        environment.set("CORDY_TOKEN", "mat_test-token");

        let upload = Cli::try_parse_from(["cordy", "attachment", "upload", "chart[v2].png"])
            .expect("attachment upload CLI");
        let uploaded = run_with_input(&upload, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("upload attachment");
        assert_eq!(uploaded.stderr, "Uploaded: chart[v2].png\n");
        let uploaded_json: Value = serde_json::from_str(&uploaded.stdout).expect("upload JSON");
        assert_eq!(uploaded_json["id"], "attachment-1");
        assert_eq!(
            uploaded_json["markdown"],
            r#"![chart\[v2\].png](/api/attachments/attachment-1/download)"#
        );

        let outside = tempfile::tempdir().expect("outside directory");
        let outside_path = outside.path().join("secret.txt");
        fs::write(&outside_path, b"must not upload").expect("outside file");
        let rejected = Cli::try_parse_from([
            "cordy",
            "attachment",
            "upload",
            outside_path.to_str().expect("outside path"),
        ])
        .expect("external attachment CLI");
        let error = run_with_input(&rejected, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect_err("task attachment outside workdir must fail closed");
        assert!(error
            .to_string()
            .contains("resolves outside the current working directory"));

        let download = Cli::try_parse_from([
            "cordy",
            "attachment",
            "download",
            "attachment-1",
            "-o",
            "attachments",
        ])
        .expect("attachment download CLI");
        let downloaded =
            run_with_input(&download, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("download attachment");
        let destination = cwd.path().join("attachments/report.txt");
        assert_eq!(
            fs::read_to_string(&destination).expect("downloaded file"),
            "attachment body"
        );
        assert!(downloaded
            .stderr
            .contains(destination.to_string_lossy().as_ref()));
        let downloaded_json: Value =
            serde_json::from_str(&downloaded.stdout).expect("download JSON");
        assert_eq!(downloaded_json["filename"], "report.txt");
        assert_eq!(downloaded_json["size"], "15");
        assert!(!downloaded.stdout.contains("../"));
        server.abort();
    }

    #[tokio::test]
    async fn repo_registry_add_remove_and_list_match_go_patch_contracts() {
        let repos = Arc::new(Mutex::new(vec![WorkspaceRepo {
            url: "https://git.example.com/web.git".into(),
            description: "web".into(),
        }]));
        let repos_get = Arc::clone(&repos);
        let repos_patch = Arc::clone(&repos);
        let app = Router::new().route(
            "/api/workspaces/ws-1",
            get(move || {
                let repos = Arc::clone(&repos_get);
                async move {
                    Json(serde_json::json!({
                        "id":"ws-1","repos":repos.lock().expect("repos").clone()
                    }))
                }
            })
            .patch(move |Json(body): Json<Value>| {
                let repos = Arc::clone(&repos_patch);
                async move {
                    let updated: Vec<WorkspaceRepo> =
                        serde_json::from_value(body["repos"].clone()).expect("repo patch body");
                    *repos.lock().expect("repos") = updated.clone();
                    Json(serde_json::json!({"id":"ws-1","repos":updated}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", "ws-1");
        environment.set("CORDY_TOKEN", "token-1");

        let add = Cli::try_parse_from([
            "cordy",
            "repo",
            "add",
            "https://git.example.com/api.git",
            "https://git.example.com/api.git",
            "--url",
            "https://git.example.com/web.git",
            "--output",
            "json",
        ])
        .expect("repo add CLI");
        let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add repos");
        let added: Value = serde_json::from_str(&added.stdout).expect("add JSON");
        assert_eq!(added["added"].as_array().expect("added").len(), 1);
        assert_eq!(added["repos"].as_array().expect("repos").len(), 2);

        let remove = Cli::try_parse_from([
            "cordy",
            "repo",
            "rm",
            "https://git.example.com/web.git",
            "--output",
            "table",
        ])
        .expect("repo remove alias");
        let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("remove repo");
        assert!(removed.stdout.starts_with("REMOVED URL"));
        assert!(removed.stdout.contains("web.git"));

        let list = Cli::try_parse_from(["cordy", "repo", "list", "--output", "table"])
            .expect("repo list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list repos");
        assert!(listed.stdout.starts_with("URL"));
        assert!(listed.stdout.contains("api.git"));
        assert!(!listed.stdout.contains("web.git"));
        server.abort();
    }

    #[test]
    fn repo_registry_rejects_empty_duplicate_and_invalid_description_inputs() {
        assert_eq!(
            repo_urls(&[" a ".into()], &["a".into(), "b".into()]).expect("dedupe"),
            vec!["a", "b"]
        );
        assert!(repo_urls(&[], &[])
            .expect_err("missing URL")
            .to_string()
            .contains("at least one"));
        assert!(repo_urls(&[" ".into()], &[])
            .expect_err("empty URL")
            .to_string()
            .contains("cannot be empty"));
        assert!(Cli::try_parse_from([
            "cordy",
            "repo",
            "remove",
            "https://git.example.com/a.git",
            "--description",
            "x"
        ])
        .is_err());
    }

    #[tokio::test]
    async fn repo_checkout_forwards_task_context_and_retries_only_marked_busy() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_handler = Arc::clone(&attempts);
        let app = Router::new().route(
            "/repo/checkout",
            post(move |request: Request| {
                let attempts = Arc::clone(&attempts_handler);
                async move {
                    let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert_eq!(
                        request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer mat_checkout")
                    );
                    let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("checkout body");
                    let body: Value = serde_json::from_slice(&body).expect("checkout JSON");
                    assert_eq!(body["url"], "https://github.com/acme/cordy.git");
                    assert_eq!(body["workspace_id"], "ws-1");
                    assert_eq!(body["agent_name"], "Rust Agent");
                    assert_eq!(body["task_id"], "task-1");
                    assert_eq!(body["checkout_mode"], "isolated");
                    assert_eq!(body["ref"], "release/v2");
                    assert_eq!(body["retry_busy"], true);
                    if attempt == 0 {
                        let mut response = axum::response::Response::builder()
                            .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                            .header("X-Cordy-Retryable", "repo-busy")
                            .header("Retry-After", "0")
                            .body(axum::body::Body::from("busy"))
                            .expect("busy response");
                        response
                            .headers_mut()
                            .insert("content-type", "text/plain".parse().expect("content type"));
                        return response;
                    }
                    axum::response::Response::builder()
                        .status(axum::http::StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"path":"/work/cordy","branch_name":"agent/rust/task-1"}"#,
                        ))
                        .expect("success response")
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("address").port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_DAEMON_PORT", port.to_string());
        environment.set("CORDY_WORKSPACE_ID", "ws-1");
        environment.set("CORDY_AGENT_NAME", "Rust Agent");
        environment.set("CORDY_TASK_ID", "task-1");
        environment.set("CORDY_TOKEN", "mat_checkout");
        environment.set("CORDY_REPO_CHECKOUT_MODE", " isolated ");
        let cli = Cli::try_parse_from([
            "cordy",
            "repo",
            "checkout",
            "https://github.com/acme/cordy.git",
            "--ref",
            "release/v2",
        ])
        .expect("repo checkout CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("repo checkout");
        assert_eq!(output.stdout, "/work/cordy\n");
        assert!(output.stderr.contains("branch: agent/rust/task-1"));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[test]
    fn repo_checkout_retry_delay_matches_go_seconds_date_and_caps() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-24T00:00:00Z")
            .expect("now")
            .with_timezone(&chrono::Utc);
        assert_eq!(
            repo_checkout_retry_delay("7", now),
            std::time::Duration::from_secs(7)
        );
        assert_eq!(
            repo_checkout_retry_delay("60", now),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            repo_checkout_retry_delay("Mon, 24 Aug 2026 00:00:05 GMT", now),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            repo_checkout_retry_delay("invalid", now),
            std::time::Duration::from_secs(1)
        );
    }

    #[test]
    fn property_read_parser_and_table_match_go_registry_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "property",
            "list",
            "--include-archived",
            "--output",
            "json",
        ])
        .expect("property list CLI");
        let Command::Property(PropertyArgs {
            command:
                PropertyCommand::List {
                    output,
                    include_archived,
                },
        }) = &cli.command
        else {
            panic!("expected property list");
        };
        assert_eq!(*output, OutputFormat::Json);
        assert!(*include_archived);

        let properties: Vec<PropertyDefinition> = serde_json::from_value(serde_json::json!([{
            "id":"11111111-1111-1111-1111-111111111111",
            "name":"Severity","type":"select","icon":"shield",
            "config":{"options":[{"id":"option-1","name":"Critical","color":"#ef4444"}]},
            "usage_count":7,"archived":true
        }]))
        .expect("property definitions");
        let table =
            format_property_definitions(&properties, OutputFormat::Table).expect("property table");
        assert!(table.starts_with("ID"));
        assert!(table.contains("11111111-1111-1111-1111-111111111111"));
        assert!(table.contains("shield"));
        assert!(table.contains("Critical"));
        assert!(table.contains("7"));
        assert!(table.contains("yes"));
    }

    #[tokio::test]
    async fn property_list_and_get_preserve_archive_query_and_full_json_fields() {
        let app = Router::new().route(
            "/api/properties",
            get(|request: Request| async move {
                let include_archived = request
                    .uri()
                    .query()
                    .is_some_and(|query| query == "include_archived=true");
                let properties = if include_archived {
                    vec![serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Severity","type":"select","description":"Impact",
                        "icon":"shield","config":{"options":[{
                            "id":"option-1","name":"Critical","color":"#ef4444"
                        }]},"position":1.5,"archived":true,"usage_count":7,
                        "created_at":"2026-08-24T00:00:00Z","updated_at":"2026-08-24T01:00:00Z"
                    })]
                } else {
                    Vec::new()
                };
                Json(serde_json::json!({"properties":properties}))
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

        let list = Cli::try_parse_from(["cordy", "property", "list", "--output", "json"])
            .expect("property list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list active properties");
        assert_eq!(
            serde_json::from_str::<Value>(&listed.stdout).expect("properties JSON"),
            serde_json::json!([])
        );

        let get = Cli::try_parse_from(["cordy", "property", "get", "severity", "--output", "json"])
            .expect("property get CLI");
        let got = run_with_input(&get, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("get archived property by name");
        let property: Value = serde_json::from_str(&got.stdout).expect("property JSON");
        assert_eq!(property["name"], "Severity");
        assert_eq!(property["description"], "Impact");
        assert_eq!(property["config"]["options"][0]["color"], "#ef4444");
        assert_eq!(property["position"], 1.5);
        assert_eq!(property["usage_count"], 7);
        assert_eq!(property["archived"], true);
        task.abort();
    }

    #[test]
    fn property_mutation_parser_preserves_option_ids_and_clear_values() {
        let cli = Cli::try_parse_from([
            "cordy",
            "property",
            "update",
            "Severity",
            "--description=",
            "--icon=",
            "--option",
            "critical:#ef4444",
            "--option",
            "Minor",
        ])
        .expect("property update CLI");
        let Command::Property(PropertyArgs {
            command: PropertyCommand::Update(args),
        }) = &cli.command
        else {
            panic!("expected property update");
        };
        assert_eq!(args.description.as_deref(), Some(""));
        assert_eq!(args.icon.as_deref(), Some(""));
        let existing = vec![PropertyOption {
            id: "option-1".into(),
            name: "Critical".into(),
            color: "#000000".into(),
        }];
        assert_eq!(
            parse_property_options(&args.option, &existing),
            vec![
                serde_json::json!({"id":"option-1","name":"critical","color":"#ef4444"}),
                serde_json::json!({"name":"Minor","color":"#6b7280"})
            ]
        );
    }

    #[tokio::test]
    async fn property_create_update_and_archive_use_go_patch_and_output_contracts() {
        let property_id = "11111111-1111-1111-1111-111111111111";
        let definition = move || {
            serde_json::json!({
                "id":property_id,"name":"Severity","type":"select","description":"",
                "icon":"shield","config":{"options":[{
                    "id":"option-1","name":"Critical","color":"#ef4444"
                }]},"position":1,"archived":false,"usage_count":0,
                "created_at":"","updated_at":""
            })
        };
        let app = Router::new()
            .route(
                "/api/properties",
                get(move || async move {
                    Json(serde_json::json!({"properties":[definition()]}))
                })
                .post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["name"], "Severity");
                    assert_eq!(body["type"], "select");
                    assert_eq!(body["description"], "");
                    assert_eq!(body["config"]["options"][0]["color"], "#ef4444");
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Severity","type":"select","description":"","icon":"shield",
                        "config":{"options":[{"id":"option-1","name":"Critical","color":"#ef4444"}]},
                        "position":1,"archived":false,"usage_count":0,"created_at":"","updated_at":""
                    }))
                }),
            )
            .route(
                "/api/properties/11111111-1111-1111-1111-111111111111",
                patch(|Json(body): Json<Value>| async move {
                    if let Some(archived) = body.get("archived") {
                        return Json(serde_json::json!({
                            "id":"11111111-1111-1111-1111-111111111111",
                            "name":"Severity","type":"select","description":"","icon":"shield",
                            "config":{"options":[]},"position":1,"archived":archived,
                            "usage_count":0,"created_at":"","updated_at":""
                        }));
                    }
                    assert_eq!(body["description"], "Impact");
                    assert_eq!(body["config"]["options"][0]["id"], "option-1");
                    Json(serde_json::json!({
                        "id":"11111111-1111-1111-1111-111111111111",
                        "name":"Severity","type":"select","description":"Impact","icon":"shield",
                        "config":body["config"],"position":1,"archived":false,
                        "usage_count":0,"created_at":"","updated_at":""
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

        let create = Cli::try_parse_from([
            "cordy",
            "property",
            "create",
            "--name",
            "Severity",
            "--type",
            "select",
            "--icon",
            "shield",
            "--option",
            "Critical:#ef4444",
            "--output",
            "json",
        ])
        .expect("property create CLI");
        let created = run_with_input(&create, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("create property");
        assert_eq!(
            serde_json::from_str::<Value>(&created.stdout).expect("created property")["id"],
            property_id
        );

        let update = Cli::try_parse_from([
            "cordy",
            "property",
            "update",
            "severity",
            "--description",
            "Impact",
            "--option",
            "Critical:#22c55e",
            "--output",
            "table",
        ])
        .expect("property update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update property");
        assert!(updated.stdout.starts_with("Property \"Severity\" updated."));
        assert!(updated.stdout.contains("Critical"));

        let archive = Cli::try_parse_from([
            "cordy", "property", "archive", "Severity", "--output", "table",
        ])
        .expect("property archive CLI");
        let archived = run_with_input(&archive, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("archive property");
        assert_eq!(archived.stdout, "Property \"Severity\" archived.\n");
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
    fn workspace_member_parser_and_role_validation_match_go_contract() {
        let cli = Cli::try_parse_from([
            "cordy",
            "workspace",
            "member",
            "invite",
            "ADA@EXAMPLE.COM",
            "alpha",
            "--role",
            "ADMIN",
            "--output",
            "json",
        ])
        .expect("workspace member invite CLI");
        let Command::Workspace(WorkspaceArgs {
            command:
                WorkspaceCommand::Member(WorkspaceMemberArgs {
                    command: WorkspaceMemberCommand::Invite(args),
                }),
        }) = &cli.command
        else {
            panic!("expected workspace member invite");
        };
        assert_eq!(args.workspace.as_deref(), Some("alpha"));
        assert_eq!(
            normalize_workspace_invite_role(&args.role).expect("admin"),
            "admin"
        );
        assert!(normalize_workspace_invite_role("owner")
            .expect_err("owner rejected")
            .to_string()
            .contains("cannot invite as owner"));
        assert!(normalize_workspace_invite_role("viewer")
            .expect_err("unknown role")
            .to_string()
            .contains("expected member or admin"));
    }

    #[tokio::test]
    async fn workspace_member_list_and_invite_use_go_http_and_output_contracts() {
        let workspace_id = "55555555-5555-5555-5555-555555555555";
        let app = Router::new().route(
            "/api/workspaces/55555555-5555-5555-5555-555555555555/members",
            get(|| async {
                Json(vec![serde_json::json!({
                    "user_id":"user-1","name":"Ada","email":"ada@example.com","role":"admin"
                })])
            })
            .post(|Json(body): Json<Value>| async move {
                assert_eq!(
                    body,
                    serde_json::json!({
                        "email":"new@example.com","role":"member"
                    })
                );
                Json(serde_json::json!({
                    "invitee_email":"new@example.com","role":"member","status":"pending"
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", workspace_id);
        environment.set("CORDY_TOKEN", "token-1");

        let list = Cli::try_parse_from([
            "cordy",
            "workspace",
            "member",
            "list",
            workspace_id,
            "--output",
            "table",
        ])
        .expect("workspace member list CLI");
        let listed = run_with_input(&list, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("list workspace members");
        assert!(listed.stdout.starts_with("USER ID"));
        assert!(listed.stdout.contains("ada@example.com"));
        assert!(listed.stdout.contains("admin"));

        let invite = Cli::try_parse_from([
            "cordy",
            "workspace",
            "member",
            "invite",
            " NEW@EXAMPLE.COM ",
            workspace_id,
        ])
        .expect("workspace member invite CLI");
        let invited = run_with_input(&invite, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("invite workspace member");
        assert_eq!(
            invited.stdout,
            "Invitation sent to new@example.com (role: member, status: pending)\n"
        );
        server.abort();
    }

    #[tokio::test]
    async fn workspace_switch_verifies_access_and_atomically_updates_only_current_profile() {
        let workspace_id = "55555555-5555-5555-5555-555555555555";
        let app = Router::new().route(
            "/api/workspaces",
            get(move || async move {
                Json(vec![serde_json::json!({
                    "id":workspace_id,"name":"Alpha","slug":"alpha"
                })])
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            r#"{"server_url":"old","unknown":{"keep":true}}"#,
        )
        .expect("profile config");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_TOKEN", "token-1");
        let cli =
            Cli::try_parse_from(["cordy", "--profile", "dev", "workspace", "switch", "ALPHA"])
                .expect("workspace switch CLI");
        let output = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("switch workspace");
        assert_eq!(
            output.stdout,
            format!("Switched to workspace: Alpha ({workspace_id})\n")
        );
        let document: Value = serde_json::from_slice(
            &fs::read(profile_dir.join("config.json")).expect("updated profile config"),
        )
        .expect("profile JSON");
        assert_eq!(document["workspace_id"], workspace_id);
        assert_eq!(document["unknown"]["keep"], true);
        assert!(!home.path().join(".cordy/config.json").exists());
        server.abort();
    }

    #[tokio::test]
    async fn workspace_mcp_list_drops_secret_config_in_every_output_format() {
        let workspace_id = "55555555-5555-5555-5555-555555555555";
        let app = Router::new().route(
            "/api/workspaces/55555555-5555-5555-5555-555555555555/mcp-servers",
            get(|| async {
                Json(vec![serde_json::json!({
                    "id":"server-1","name":"linear","transport":"http",
                    "url":"https://secret.example/token","headers":{"Authorization":"Bearer secret"},
                    "config":{"env":{"API_KEY":"secret"}}
                })])
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", workspace_id);
        environment.set("CORDY_TOKEN", "token-1");

        for output in ["json", "table"] {
            let cli = Cli::try_parse_from([
                "cordy",
                "workspace",
                "mcp",
                "list",
                workspace_id,
                "--output",
                output,
            ])
            .expect("workspace mcp list CLI");
            let listed = run_with_input(&cli, &environment, &mut Cursor::new(Vec::<u8>::new()))
                .await
                .expect("list workspace MCP servers");
            assert!(listed.stdout.contains("linear"));
            assert!(listed.stdout.contains("http"));
            assert!(!listed.stdout.contains("secret"));
            assert!(!listed.stdout.contains("Authorization"));
            assert!(!listed.stdout.contains("API_KEY"));
        }
        server.abort();
    }

    #[test]
    fn workspace_mcp_config_validation_is_secret_safe_and_rejects_non_objects() {
        let secret = r#"{"token":"sk-do-not-echo""#;
        let error = parse_workspace_mcp_server_config(secret).expect_err("invalid JSON");
        assert_eq!(
            error.to_string(),
            "--server-config must be a valid JSON object"
        );
        assert!(!error.to_string().contains("sk-do-not-echo"));
        assert_eq!(
            parse_workspace_mcp_server_config("null")
                .expect_err("null")
                .to_string(),
            "--server-config must be a JSON object, not null"
        );
        assert!(parse_workspace_mcp_server_config("[]")
            .expect_err("array")
            .to_string()
            .contains("must be a JSON object"));
    }

    #[tokio::test]
    async fn workspace_mcp_mutations_use_safe_inputs_and_never_echo_config() {
        let workspace_id = "55555555-5555-5555-5555-555555555555";
        let endpoint = "/api/workspaces/55555555-5555-5555-5555-555555555555/mcp-servers";
        let resource = "/api/workspaces/55555555-5555-5555-5555-555555555555/mcp-servers/server-1";
        let app = Router::new()
            .route(
                endpoint,
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["name"], "linear");
                    assert_eq!(body["config"]["url"], "https://linear.example");
                    Json(serde_json::json!({
                        "id":"server-1","name":"linear","transport":"http",
                        "config":{"url":"https://secret.example","headers":{"Authorization":"secret"}}
                    }))
                }),
            )
            .route(
                resource,
                put(|Json(body): Json<Value>| async move {
                    assert_eq!(body["name"], "linear-v2");
                    assert!(body.get("config").is_none());
                    Json(serde_json::json!({
                        "id":"server-1","name":"linear-v2","transport":"stdio",
                        "url":"https://secret.example"
                    }))
                })
                .delete(|| async { axum::http::StatusCode::NO_CONTENT }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        fs::write(
            cwd.path().join("linear.json"),
            r#"{"url":"https://linear.example"}"#,
        )
        .expect("MCP config file");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("CORDY_SERVER_URL", format!("http://{address}"));
        environment.set("CORDY_WORKSPACE_ID", workspace_id);
        environment.set("CORDY_TOKEN", "token-1");

        let add = Cli::try_parse_from([
            "cordy",
            "workspace",
            "mcp",
            "add",
            "linear",
            workspace_id,
            "--server-config-file",
            "linear.json",
        ])
        .expect("workspace MCP add CLI");
        let added = run_with_input(&add, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("add workspace MCP server");
        assert!(added.stdout.contains("linear"));
        assert!(!added.stdout.contains("secret"));
        assert!(!added.stdout.contains("config"));

        let update = Cli::try_parse_from([
            "cordy",
            "workspace",
            "mcp",
            "update",
            "server-1",
            workspace_id,
            "--name",
            " linear-v2 ",
            "--output",
            "table",
        ])
        .expect("workspace MCP update CLI");
        let updated = run_with_input(&update, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("update workspace MCP server");
        assert!(updated.stdout.contains("linear-v2"));
        assert!(!updated.stdout.contains("secret"));

        let remove = Cli::try_parse_from([
            "cordy",
            "workspace",
            "mcp",
            "remove",
            "server-1",
            workspace_id,
        ])
        .expect("workspace MCP remove CLI");
        let removed = run_with_input(&remove, &environment, &mut Cursor::new(Vec::<u8>::new()))
            .await
            .expect("remove workspace MCP server");
        assert_eq!(removed.stdout, "removed MCP server server-1\n");
        server.abort();
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
