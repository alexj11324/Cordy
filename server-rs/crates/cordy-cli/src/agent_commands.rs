use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;
use url::{form_urlencoded, Url};

use super::{
    apply_agent_permission_args, copied_agent_max_concurrent_tasks, format_agent_details_table,
    format_agent_list_table, format_table, format_workspace_mcp_servers, new_api_client,
    required_workspace_id, value_string, AgentCopyArgs, AgentCreateArgs, AgentMcpListArgs,
    AgentMcpMutationArgs, AgentUpdateArgs, Cli, Environment, OutputFormat, RunOutput,
    WorkspaceMcpServer,
};

pub(super) async fn run_agent_list(
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

pub(super) async fn run_agent_get(
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

#[derive(Clone, Copy)]
pub(super) enum AgentMcpAction {
    Add,
    Enable,
    Disable,
    Remove,
}

pub(super) fn agent_mcp_path(agent_id: &str, suffix: &[&str]) -> String {
    let mut url = Url::parse("http://localhost").expect("constant URL");
    {
        let mut segments = url.path_segments_mut().expect("hierarchical URL");
        segments.clear();
        segments.extend(["api", "agents", agent_id.trim(), "mcp-servers"]);
        segments.extend(suffix.iter().copied());
    }
    url.path().into()
}

pub(super) async fn run_agent_mcp_list(
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

pub(super) async fn run_agent_mcp_mutation(
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

pub(super) async fn run_agent_copy<R: Read>(
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
