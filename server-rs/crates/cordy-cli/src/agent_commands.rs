use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use url::form_urlencoded;

use super::{
    apply_agent_permission_args, format_agent_details_table, format_agent_list_table, format_table,
    http_timeout, new_api_client, required_workspace_id, resolve_agent_secret_json,
    validate_agent_custom_env, value_string, AgentCreateArgs, AgentUpdateArgs, Cli, Environment,
    OutputFormat, RunOutput,
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

pub(super) async fn run_agent_lifecycle(
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

pub(super) async fn run_agent_tasks(
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

pub(super) async fn run_agent_avatar(
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
