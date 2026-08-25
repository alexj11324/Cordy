use super::{
    apply_agent_permission_args, new_api_client, resolve_agent_secret_json,
    validate_agent_custom_env, value_string, AgentCreateArgs, AgentUpdateArgs, Cli, Environment,
    OutputFormat, RunOutput,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;

pub(super) async fn run_agent_create<R: Read>(
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

pub(super) async fn run_agent_update<R: Read>(
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
