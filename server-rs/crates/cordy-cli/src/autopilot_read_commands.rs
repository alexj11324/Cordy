use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    Cli, Environment, OutputFormat, RunOutput, format_autopilot_runs_table, format_autopilot_table,
    load_autopilot_agent_names, new_api_client, required_workspace_id, resolve_autopilot_id,
    resolve_current_workspace_id,
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
        .map_err(|error| {
            if error
                .to_string()
                .starts_with("ambiguous autopilot id prefix")
            {
                error
            } else {
                anyhow::anyhow!("resolve autopilot: {error:#}")
            }
        })?;
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

#[derive(Debug, Deserialize, Serialize)]
struct AutopilotRunsEnvelope {
    runs: Vec<Value>,
    total: i64,
}

pub(super) async fn run_autopilot_runs(
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
