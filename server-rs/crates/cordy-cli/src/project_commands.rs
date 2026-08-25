use anyhow::{Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    format_project_details_table, format_project_list_table, load_issue_actor_names,
    new_api_client, project_actor_inputs, resolve_current_workspace_id, resolve_issue_project_id,
    Cli, Environment, OutputFormat, RunOutput,
};

pub(super) async fn run_project_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    full_id: bool,
    status: Option<&str>,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if !workspace_id.is_empty() {
        serializer.append_pair("workspace_id", &workspace_id);
    }
    if let Some(status) = status.filter(|status| !status.is_empty()) {
        serializer.append_pair("status", status);
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        "/api/projects".into()
    } else {
        format!("/api/projects?{query}")
    };
    let result: Value = client.get_json(&path).await.context("list projects")?;
    let projects = result
        .get("projects")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(projects)?),
        OutputFormat::Table => {
            let inputs = project_actor_inputs(projects);
            let actors = load_issue_actor_names(&client, &workspace_id, &inputs).await;
            format_project_list_table(projects, &actors, full_id)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_project_get(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let project: Value = client
        .get_json(&format!("/api/projects/{project_id}"))
        .await
        .context("get project")?;
    let resource_count = project
        .get("resource_count")
        .and_then(Value::as_f64)
        .unwrap_or_default() as i64;
    let stderr = if resource_count > 0 {
        format!(
            "{resource_count} resource(s) attached — run `cordy project resource list {project_id}` to view.\n"
        )
    } else {
        String::new()
    };
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&project)?),
        OutputFormat::Table => {
            let inputs = project_actor_inputs(std::slice::from_ref(&project));
            let actors = load_issue_actor_names(&client, &workspace_id, &inputs).await;
            format_project_details_table(&project, &actors)
        }
    };
    Ok(RunOutput { stdout, stderr })
}
