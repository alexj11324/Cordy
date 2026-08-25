use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::project_status_commands::validate_project_status;
use super::{
    format_table, new_api_client, resolve_current_workspace_id, resolve_issue_project_id,
    resolve_project_reference, resolve_subscriber_name, value_string, ApiClient, Cli, Environment,
    OutputFormat, ProjectCreateArgs, ProjectUpdateArgs, ResolvedIssueAssignee, RunOutput,
};

pub(super) fn format_project_mutation(project: &Value, output: OutputFormat) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(project)?)),
        OutputFormat::Table => Ok(format_table(&[
            vec!["ID".into(), "TITLE".into(), "STATUS".into()],
            vec![
                value_string(project, "id"),
                value_string(project, "title"),
                value_string(project, "status"),
            ],
        ])),
    }
}

async fn resolve_project_lead(
    client: &ApiClient,
    workspace_id: &str,
    lead: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_subscriber_name(client, workspace_id, lead)
        .await
        .map_err(|error| anyhow::anyhow!("resolve lead: {error}"))
}

pub(super) async fn run_project_create(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectCreateArgs,
) -> Result<RunOutput> {
    let title = args
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .context("--title is required")?;
    if let Some(status) = args.status.as_deref().filter(|status| !status.is_empty()) {
        validate_project_status(status)?;
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let mut body = serde_json::Map::from_iter([("title".into(), Value::String(title.into()))]);
    for (key, value) in [
        ("description", args.description.as_deref()),
        ("status", args.status.as_deref()),
        ("icon", args.icon.as_deref()),
        ("start_date", args.start_date.as_deref()),
        ("due_date", args.due_date.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            body.insert(key.into(), Value::String(value.into()));
        }
    }
    if let Some(lead) = args.lead.as_deref().filter(|lead| !lead.is_empty()) {
        let lead = resolve_project_lead(&client, &workspace_id, lead).await?;
        body.insert("lead_type".into(), Value::String(lead.actor_type));
        body.insert("lead_id".into(), Value::String(lead.id));
    }
    let resources = args
        .repo
        .iter()
        .map(|repo| repo.trim())
        .filter(|repo| !repo.is_empty())
        .map(|repo| {
            serde_json::json!({
                "resource_type":"github_repo",
                "resource_ref":{"url":repo}
            })
        })
        .collect::<Vec<_>>();
    if !resources.is_empty() {
        body.insert("resources".into(), Value::Array(resources));
    }
    let project: Value = client
        .post_json("/api/projects", &body)
        .await
        .context("create project")?;
    Ok(RunOutput {
        stdout: format_project_mutation(&project, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_project_update(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectUpdateArgs,
) -> Result<RunOutput> {
    if let Some(status) = &args.status {
        validate_project_status(status)?;
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, &args.id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("title", args.title.as_ref()),
        ("description", args.description.as_ref()),
        ("status", args.status.as_ref()),
        ("icon", args.icon.as_ref()),
        ("start_date", args.start_date.as_ref()),
        ("due_date", args.due_date.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(lead) = &args.lead {
        let lead = resolve_project_lead(&client, &workspace_id, lead).await?;
        body.insert("lead_type".into(), Value::String(lead.actor_type));
        body.insert("lead_id".into(), Value::String(lead.id));
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --status, --description, --icon, --lead, --start-date, --due-date"
        );
    }
    let project: Value = client
        .put_json(&format!("/api/projects/{project_id}"), &body)
        .await
        .context("update project")?;
    Ok(RunOutput {
        stdout: format_project_mutation(&project, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_project_delete(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    _output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (project_id, display) = resolve_project_reference(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    client
        .delete(&format!("/api/projects/{project_id}"))
        .await
        .context("delete project")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Project {display} deleted.\n"),
    })
}
