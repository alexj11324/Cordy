use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::project_resource_support::{
    build_project_resource_add_ref, build_project_resource_update_ref, find_project_resource,
    project_resources, resolve_project_resource_reference,
};
use super::{
    display_id, format_table, new_api_client, resolve_current_workspace_id,
    resolve_issue_project_id, resolve_project_reference, value_string, Cli, Environment,
    OutputFormat, ProjectResourceAddArgs, ProjectResourceUpdateArgs, RunOutput,
};

pub(super) fn summarize_project_resource_ref(resource_ref: &Value) -> String {
    let Some(object) = resource_ref.as_object() else {
        return String::new();
    };
    let url = value_string(resource_ref, "url");
    if !url.is_empty() {
        let checkout_ref = value_string(resource_ref, "ref");
        return if checkout_ref.trim().is_empty() {
            url
        } else {
            format!("{url} @ {}", checkout_ref.trim())
        };
    }
    let local_path = value_string(resource_ref, "local_path");
    if !local_path.is_empty() {
        return local_path;
    }
    serde_json::to_string(object).unwrap_or_default()
}

pub(super) fn format_project_resources(resources: &[Value], full_id: bool) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TYPE".into(),
        "REF".into(),
        "LABEL".into(),
    ]];
    rows.extend(resources.iter().map(|resource| {
        vec![
            display_id(&value_string(resource, "id"), full_id),
            value_string(resource, "resource_type"),
            summarize_project_resource_ref(resource.get("resource_ref").unwrap_or(&Value::Null)),
            value_string(resource, "label"),
        ]
    }));
    format_table(&rows)
}

pub(super) async fn run_project_resource_list(
    cli: &Cli,
    environment: &Environment,
    project: &str,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, project)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let result: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("list project resources")?;
    let resources = project_resources(&result);
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(resources)?),
            OutputFormat::Table => format_project_resources(resources, full_id),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_project_resource_add(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectResourceAddArgs,
) -> Result<RunOutput> {
    let resource_type = args.resource_type.trim();
    let resource_ref = build_project_resource_add_ref(args)?;
    let mut body = serde_json::Map::from_iter([
        ("resource_type".into(), Value::String(resource_type.into())),
        ("resource_ref".into(), resource_ref),
    ]);
    if let Some(label) = args.label.as_deref().filter(|label| !label.is_empty()) {
        body.insert("label".into(), Value::String(label.into()));
    }
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, &args.project_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let resource: Value = client
        .post_json(&format!("/api/projects/{project_id}/resources"), &body)
        .await
        .context("add project resource")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&resource)?),
        OutputFormat::Table => format_table(&[
            vec!["ID".into(), "TYPE".into(), "REF".into()],
            vec![
                value_string(&resource, "id"),
                value_string(&resource, "resource_type"),
                summarize_project_resource_ref(
                    resource.get("resource_ref").unwrap_or(&Value::Null),
                ),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_project_resource_update(
    cli: &Cli,
    environment: &Environment,
    args: &ProjectResourceUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, &args.project_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let (resource_id, _) =
        resolve_project_resource_reference(&client, &project_id, &args.resource_id)
            .await
            .map_err(|error| anyhow::anyhow!("resolve project resource: {error}"))?;
    let existing: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("list project resources")?;
    let existing = find_project_resource(project_resources(&existing), &resource_id);
    let resource_type = existing
        .map(|resource| value_string(resource, "resource_type"))
        .unwrap_or_default();
    let existing_ref = existing
        .and_then(|resource| resource.get("resource_ref"))
        .and_then(Value::as_object);
    let mut body = serde_json::Map::new();
    if let Some(resource_ref) =
        build_project_resource_update_ref(args, &resource_type, existing_ref)?
    {
        body.insert("resource_ref".into(), resource_ref);
    }
    if args.clear_label {
        body.insert("label".into(), Value::Null);
    } else if let Some(label) = &args.label {
        body.insert("label".into(), Value::String(label.clone()));
    }
    if let Some(position) = args.position {
        body.insert("position".into(), Value::from(position));
    }
    if body.is_empty() {
        bail!(
            "nothing to update — pass --ref / --url / --local-path / --label / --position / --clear-label"
        );
    }
    let resource: Value = client
        .put_json(
            &format!("/api/projects/{project_id}/resources/{resource_id}"),
            &body,
        )
        .await
        .context("update project resource")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&resource)?),
            OutputFormat::Table => format_table(&[
                vec!["ID".into(), "TYPE".into(), "REF".into(), "LABEL".into()],
                vec![
                    value_string(&resource, "id"),
                    value_string(&resource, "resource_type"),
                    summarize_project_resource_ref(
                        resource.get("resource_ref").unwrap_or(&Value::Null),
                    ),
                    value_string(&resource, "label"),
                ],
            ]),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_project_resource_remove(
    cli: &Cli,
    environment: &Environment,
    project: &str,
    resource: &str,
    _output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let (project_id, project_display) = resolve_project_reference(&client, &workspace_id, project)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let (resource_id, resource_display) =
        resolve_project_resource_reference(&client, &project_id, resource)
            .await
            .map_err(|error| anyhow::anyhow!("resolve project resource: {error}"))?;
    client
        .delete(&format!(
            "/api/projects/{project_id}/resources/{resource_id}"
        ))
        .await
        .context("remove project resource")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Resource {resource_display} removed from project {project_display}.\n"),
    })
}
