use anyhow::{bail, Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    display_id, format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_project_id, resolve_project_reference, resolve_subscriber_name, value_string,
    ApiClient, Cli, Environment, IssueActorNames, OutputFormat, ProjectCreateArgs,
    ProjectUpdateArgs, ResolvedIssueExecutor, RunOutput,
};

pub(super) fn project_lead(project: &Value, actors: &IssueActorNames) -> String {
    let actor_type = value_string(project, "lead_type");
    let actor_id = value_string(project, "lead_id");
    if actor_type.is_empty() || actor_id.is_empty() {
        return String::new();
    }
    let key = format!("{actor_type}:{actor_id}");
    actors
        .0
        .get(&key)
        .map_or(key, |name| format!("{actor_type}:{name}"))
}

pub(super) fn project_actor_inputs(projects: &[Value]) -> Vec<Value> {
    projects
        .iter()
        .map(|project| {
            serde_json::json!({
                "executor_type":project.get("lead_type").cloned().unwrap_or(Value::Null),
                "executor_id":project.get("lead_id").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

pub(super) fn format_project_list_table(
    projects: &[Value],
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TITLE".into(),
        "STATUS".into(),
        "LEAD".into(),
        "CREATED".into(),
    ]];
    rows.extend(projects.iter().map(|project| {
        vec![
            display_id(&value_string(project, "id"), full_id),
            value_string(project, "title"),
            value_string(project, "status"),
            project_lead(project, actors),
            value_string(project, "created_at")
                .chars()
                .take(10)
                .collect(),
        ]
    }));
    format_table(&rows)
}

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

pub(super) fn format_project_details_table(project: &Value, actors: &IssueActorNames) -> String {
    format_table(&[
        vec![
            "ID".into(),
            "TITLE".into(),
            "STATUS".into(),
            "LEAD".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            value_string(project, "id"),
            value_string(project, "title"),
            value_string(project, "status"),
            project_lead(project, actors),
            value_string(project, "description"),
        ],
    ])
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
            "{resource_count} resource(s) attached — run `patchbay project resource list {project_id}` to view.\n"
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

pub(super) const PROJECT_STATUSES: &[&str] =
    &["planned", "in_progress", "paused", "completed", "cancelled"];

pub(super) fn validate_project_status(status: &str) -> Result<()> {
    if PROJECT_STATUSES.contains(&status) {
        Ok(())
    } else {
        bail!(
            "invalid status {status:?}; valid values: {}",
            PROJECT_STATUSES.join(", ")
        )
    }
}

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
) -> Result<ResolvedIssueExecutor> {
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

pub(super) async fn run_project_status(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    status: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    validate_project_status(status)?;
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let project: Value = client
        .put_json(
            &format!("/api/projects/{project_id}"),
            &serde_json::json!({"status":status}),
        )
        .await
        .context("update status")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&project)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!(
            "Project {} status changed to {status}.\n",
            value_string(&project, "title")
        ),
    })
}
