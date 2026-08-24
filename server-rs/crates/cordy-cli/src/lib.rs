//! Cordy CLI — incremental Rust replacement for `server/cmd/cordy`.
//!
//! The S10 migration deliberately registers only fully functional commands.
//! Shared configuration, API, error, and safe text-input behavior is ported
//! with each vertical slice rather than exposing placeholder command trees.

mod api;
pub mod config;
pub mod error;

use anyhow::{bail, Context, Result};
use api::{http_timeout, ApiClient, NetworkError};
use clap::{Args, Parser, Subcommand, ValueEnum};
use config::Environment;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use url::{form_urlencoded, Url};

pub const CLIENT_VERSION: &str = env!("CORDY_BUILD_VERSION");
pub const BUILD_COMMIT: &str = env!("CORDY_BUILD_COMMIT");
pub const BUILD_DATE: &str = env!("CORDY_BUILD_DATE");
pub const BUILD_GO_VERSION: &str = env!("CORDY_BUILD_GO_VERSION");
pub const BUILD_OS: &str = env!("CORDY_BUILD_OS");
pub const BUILD_ARCH: &str = env!("CORDY_BUILD_ARCH");
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
    #[command(about = "Print version information")]
    Version {
        #[arg(long, value_enum, default_value_t = VersionOutput::Text)]
        output: VersionOutput,
    },
}

#[derive(Debug, Args)]
struct IssueArgs {
    #[command(subcommand)]
    command: IssueCommand,
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
        Command::Issue(IssueArgs {
            command: IssueCommand::List(args),
        }) => run_issue_list(cli, environment, args).await,
        Command::Issue(IssueArgs {
            command: IssueCommand::Get { id, output },
        }) => run_issue_get(cli, environment, id, *output).await,
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
        let id = resolve_issue_assignee_id(client, workspace_id, id)
            .await
            .context("resolve assignee")?;
        params.insert("assignee_id".into(), id);
    } else if let Some(name) = &args.assignee {
        let id = resolve_issue_assignee_name(client, workspace_id, name)
            .await
            .context("resolve assignee")?;
        params.insert("assignee_id".into(), id);
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

async fn fetch_issue_actors(
    client: &ApiClient,
    workspace_id: &str,
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
    let squads = retry_actor_get::<Vec<Value>>(client, "/api/squads")
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
        });
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
) -> Result<String> {
    let input = raw.trim();
    if !is_canonical_uuid(input) {
        bail!("expected a canonical UUID, got {raw:?}");
    }
    let actors = fetch_issue_actors(client, workspace_id).await;
    if actors.iter().all(Result::is_err) {
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
        .find(|actor| actor.id.eq_ignore_ascii_case(input))
    {
        return Ok(actor.id.clone());
    }
    bail!("no member, agent, or squad found with ID {input:?}")
}

async fn resolve_issue_assignee_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<String> {
    let input = normalize_assignee_input(raw);
    if input.is_empty() {
        bail!("no member, agent, or squad found matching {raw:?}");
    }
    let actors = fetch_issue_actors(client, workspace_id).await;
    if actors.iter().all(Result::is_err) {
        let errors = actors
            .iter()
            .enumerate()
            .map(|(index, result)| {
                let kind = ["members", "agents", "squads"][index];
                format!("fetch {kind}: {}", result.as_ref().unwrap_err())
            })
            .collect::<Vec<_>>()
            .join("; ");
        bail!("failed to resolve assignee: {errors}");
    }
    let actors = actors
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flatten()
        .filter(|actor| !actor.archived)
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
            [actor] => return Ok(actor.id.clone()),
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
    bail!("no member, agent, or squad found matching {input:?}")
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
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok(input.into());
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
        [project] => Ok(value_string(project, "id")),
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
    use axum::routing::{get, patch, post};
    use axum::{Json, Router};
    use clap::Parser;
    use std::fs;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

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
