use anyhow::{bail, Context, Result};
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
