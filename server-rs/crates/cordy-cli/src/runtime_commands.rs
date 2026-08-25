use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::Value;

use super::{
    format_runtime_delete_result, format_runtime_rows, new_api_client, runtime_delete_conflict,
    value_string, Cli, Environment, OutputFormat, RunOutput,
};

pub(super) async fn run_runtime_list(
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

pub(super) async fn run_runtime_usage(
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

pub(super) async fn run_runtime_activity(
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

pub(super) async fn run_runtime_rename(
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

pub(super) async fn run_runtime_delete(
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
