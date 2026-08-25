use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;

use super::{
    format_table, new_api_client, resolve_agent_secret_json, validate_agent_custom_env,
    AgentEnvSetArgs, Cli, Environment, OutputFormat, RunOutput,
};

pub(super) async fn run_agent_env_get(
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

pub(super) async fn run_agent_env_set<R: Read>(
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
