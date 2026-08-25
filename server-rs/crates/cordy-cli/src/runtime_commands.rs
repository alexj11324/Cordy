use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{
    Cli, Environment, OutputFormat, RunOutput, format_runtime_delete_result, new_api_client,
    runtime_delete_conflict, value_string,
};

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
