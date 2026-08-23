//! Cordy CLI — incremental Rust replacement for `server/cmd/cordy`.
//!
//! The S10 migration deliberately registers only fully functional commands.
//! Shared configuration, API, error, and safe text-input behavior is ported
//! with each vertical slice rather than exposing placeholder command trees.

mod api;
pub mod config;
pub mod error;

use anyhow::{bail, Context, Result};
use api::{http_timeout, ApiClient};
use clap::{Args, Parser, Subcommand, ValueEnum};
use config::Environment;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use url::Url;

pub const CLIENT_VERSION: &str = match option_env!("CORDY_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(
    name = "cordy",
    version = CLIENT_VERSION,
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
    #[command(about = "Work with your user account")]
    User(UserArgs),
    #[command(about = "Work with workspaces")]
    Workspace(WorkspaceArgs),
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
    }
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
        ensure_file_within_workdir(path, environment.current_dir(), args.allow_external_file)?;
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
            "--description-file path {:?} resolves outside the current working directory; write agent temp files inside the task workdir (e.g. ./description.md) rather than machine-shared paths like /tmp, where another run's stale file can be read by mistake. Pass --allow-external-file to override.",
            file_path
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
    use axum::routing::{get, patch};
    use axum::{Json, Router};
    use clap::Parser;
    use std::fs;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

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
