use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use url::form_urlencoded;

use super::{
    format_autopilot_runs_table, format_autopilot_table, http_timeout, load_autopilot_agent_names,
    new_api_client, required_workspace_id, resolve_autopilot_agent, resolve_autopilot_id,
    resolve_autopilot_subscribers, resolve_current_workspace_id, resolve_project_reference,
    value_string, AutopilotCreateArgs, AutopilotUpdateArgs, Cli, Environment, OutputFormat,
    RunOutput,
};

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotListEnvelope {
    autopilots: Vec<Value>,
    total: i64,
}

pub(super) async fn run_autopilot_list(
    cli: &Cli,
    environment: &Environment,
    status: &str,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let path = if status.is_empty() {
        "/api/autopilots".into()
    } else {
        format!(
            "/api/autopilots?status={}",
            form_urlencoded::byte_serialize(status.as_bytes()).collect::<String>()
        )
    };
    let response: AutopilotListEnvelope =
        client.get_json(&path).await.context("list autopilots")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let agents =
                load_autopilot_agent_names(&client, &workspace_id, &response.autopilots).await;
            format_autopilot_table(&response.autopilots, full_id, &agents)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let response: Value = client
        .get_json(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .context("get autopilot")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
        OutputFormat::Table => {
            let autopilot = response.get("autopilot").unwrap_or(&Value::Null);
            let agents =
                load_autopilot_agent_names(&client, &workspace_id, std::slice::from_ref(autopilot))
                    .await;
            format_autopilot_table(std::slice::from_ref(autopilot), true, &agents)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
pub(super) async fn run_autopilot_create(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotCreateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = required_workspace_id(cli, environment)?;
    let title = args.title.as_deref().unwrap_or_default();
    if title.is_empty() {
        bail!("--title is required");
    }
    let agent = args.agent.as_deref().unwrap_or_default();
    if agent.is_empty() {
        bail!("--agent is required (agent name or ID)");
    }
    let mode = args.mode.as_deref().unwrap_or_default();
    if mode.is_empty() {
        bail!("--mode is required (create_issue or run_only)");
    }
    if !matches!(mode, "create_issue" | "run_only") {
        bail!("--mode must be create_issue or run_only");
    }

    let agent_id = resolve_autopilot_agent(&client, &workspace_id, agent)
        .await
        .map_err(|error| anyhow::anyhow!("resolve agent: {error:#}"))?;
    let mut body = serde_json::Map::from_iter([
        ("title".into(), Value::String(title.into())),
        ("assignee_id".into(), Value::String(agent_id)),
        ("execution_mode".into(), Value::String(mode.into())),
    ]);
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    if let Some(priority) = &args.priority {
        body.insert("priority".into(), Value::String(priority.clone()));
    }
    if !args.project.is_empty() {
        let project_id = resolve_project_reference(&client, &workspace_id, &args.project)
            .await
            .map(|(id, _)| id)
            .map_err(|error| anyhow::anyhow!("resolve project: {error:#}"))?;
        body.insert("project_id".into(), Value::String(project_id));
    }
    if !args.issue_title_template.is_empty() {
        body.insert(
            "issue_title_template".into(),
            Value::String(args.issue_title_template.clone()),
        );
    }
    if !args.subscriber.is_empty() {
        body.insert(
            "subscribers".into(),
            Value::Array(
                resolve_autopilot_subscribers(&client, &workspace_id, &args.subscriber).await?,
            ),
        );
    }

    let result: Value = client
        .post_json("/api/autopilots", &body)
        .await
        .context("create autopilot")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!(
                "Autopilot created: {} ({})\n",
                value_string(&result, "title"),
                value_string(&result, "id")
            ),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_autopilot_update(
    cli: &Cli,
    environment: &Environment,
    args: &AutopilotUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, &args.id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;

    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("title", args.title.as_ref()),
        ("description", args.description.as_ref()),
        ("priority", args.priority.as_ref()),
        ("status", args.status.as_ref()),
        ("issue_title_template", args.issue_title_template.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(agent) = &args.agent {
        let agent_id = resolve_autopilot_agent(&client, &workspace_id, agent)
            .await
            .map_err(|error| anyhow::anyhow!("resolve agent: {error:#}"))?;
        body.insert("assignee_type".into(), Value::String("agent".into()));
        body.insert("assignee_id".into(), Value::String(agent_id));
    }
    if let Some(project) = &args.project {
        let value = if project.is_empty() {
            Value::Null
        } else {
            let id = resolve_project_reference(&client, &workspace_id, project)
                .await
                .map(|(id, _)| id)
                .map_err(|error| anyhow::anyhow!("resolve project: {error:#}"))?;
            Value::String(id)
        };
        body.insert("project_id".into(), value);
    }
    if let Some(mode) = &args.mode {
        if !matches!(mode.as_str(), "create_issue" | "run_only") {
            bail!("--mode must be create_issue or run_only");
        }
        body.insert("execution_mode".into(), Value::String(mode.clone()));
    }
    if args.clear_subscribers && !args.subscriber.is_empty() {
        bail!("--subscriber and --clear-subscribers are mutually exclusive");
    }
    if args.clear_subscribers {
        body.insert("subscribers".into(), Value::Array(Vec::new()));
    } else if !args.subscriber.is_empty() {
        body.insert(
            "subscribers".into(),
            Value::Array(
                resolve_autopilot_subscribers(&client, &workspace_id, &args.subscriber).await?,
            ),
        );
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --description, --agent, --status, --mode, etc."
        );
    }

    let result: Value = client
        .patch_json(&format!("/api/autopilots/{autopilot_id}"), &body)
        .await
        .context("update autopilot")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!(
                "Autopilot updated: {} ({})\n",
                value_string(&result, "title"),
                value_string(&result, "id")
            ),
        },
        stderr: String::new(),
    })
}
async fn run_autopilot_delete(cli: &Cli, environment: &Environment, id: &str) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, display) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    client
        .delete(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .context("delete autopilot")?;
    Ok(RunOutput {
        stdout: format!("Autopilot {display} deleted.\n"),
        stderr: String::new(),
    })
}

async fn run_autopilot_trigger(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT"))
        .saturating_add(Duration::from_secs(5))
        .max(Duration::from_secs(30));
    let client = new_api_client(cli, environment)?.with_request_timeout(timeout);
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let run: Value = client
        .post_json(
            &format!("/api/autopilots/{autopilot_id}/trigger"),
            &Value::Null,
        )
        .await
        .context("trigger autopilot")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&run)?),
            OutputFormat::Table => format!(
                "Autopilot triggered: run {} (status: {})\n",
                value_string(&run, "id"),
                value_string(&run, "status")
            ),
        },
        stderr: String::new(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotRunsEnvelope {
    runs: Vec<Value>,
    total: i64,
}

async fn run_autopilot_runs(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    limit: i32,
    offset: i32,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (autopilot_id, _) = resolve_autopilot_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    if limit > 0 {
        query.append_pair("limit", &limit.to_string());
    }
    if offset > 0 {
        query.append_pair("offset", &offset.to_string());
    }
    let query = query.finish();
    let path = if query.is_empty() {
        format!("/api/autopilots/{autopilot_id}/runs")
    } else {
        format!("/api/autopilots/{autopilot_id}/runs?{query}")
    };
    let response: AutopilotRunsEnvelope = client.get_json(&path).await.context("list runs")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&response)?),
            OutputFormat::Table => format_autopilot_runs_table(&response.runs),
        },
        stderr: String::new(),
    })
}
