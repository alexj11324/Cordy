use anyhow::{bail, Context, Result};
use cordy_config::agent_concurrency;
use serde_json::Value;
use std::io::Read;

use super::{
    apply_agent_permission_args, copied_agent_max_concurrent_tasks, new_api_client,
    resolve_agent_secret_json, validate_agent_custom_env, value_string, AgentCopyArgs, Cli,
    Environment, OutputFormat, RunOutput,
};

pub(super) async fn run_agent_copy<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &AgentCopyArgs,
    input: &mut R,
) -> Result<RunOutput> {
    if let Some(value) = args.max_concurrent_tasks {
        if let Err(error) = agent_concurrency::validate_max_concurrent_tasks(value) {
            bail!("--max-concurrent-tasks {error} (got {value})");
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
