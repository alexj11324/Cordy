use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    compact_uuid, display_id, format_table, is_canonical_uuid, new_api_client,
    normalize_uuid_prefix, resolve_current_workspace_id, resolve_issue_project_id,
    resolve_project_reference, value_string, ApiClient, Cli, Environment, OutputFormat,
    ProjectResourceAddArgs, ProjectResourceUpdateArgs, RunOutput,
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

fn project_resources(result: &Value) -> &[Value] {
    result
        .get("resources")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
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

fn parse_generic_resource_ref(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|error| anyhow::anyhow!("--ref is not valid JSON: {error}"))
}

pub(super) fn build_project_resource_add_ref(args: &ProjectResourceAddArgs) -> Result<Value> {
    let resource_type = args.resource_type.trim();
    if resource_type.is_empty() {
        bail!("--type is required");
    }
    if let Some(raw) = &args.resource_ref {
        let raw = raw.trim();
        if !raw.is_empty()
            && (resource_type != "github_repo" || raw.starts_with('{') || raw.starts_with('['))
        {
            return parse_generic_resource_ref(raw);
        }
        if resource_type != "github_repo" {
            bail!("--ref must be a JSON resource_ref payload for resource type {resource_type:?}");
        }
    }
    match resource_type {
        "github_repo" => {
            let url = args
                .url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .context("github_repo requires --url (or pass a JSON payload via --ref)")?;
            let mut resource_ref = serde_json::Map::from_iter([(
                "url".into(),
                Value::String(url.into()),
            )]);
            if let Some(hint) = args
                .default_branch_hint
                .as_deref()
                .map(str::trim)
                .filter(|hint| !hint.is_empty())
            {
                resource_ref.insert("default_branch_hint".into(), Value::String(hint.into()));
            }
            if let Some(checkout_ref) = args
                .resource_ref
                .as_deref()
                .map(str::trim)
                .filter(|checkout_ref| !checkout_ref.is_empty())
            {
                resource_ref.insert("ref".into(), Value::String(checkout_ref.into()));
            }
            Ok(Value::Object(resource_ref))
        }
        "local_directory" => {
            let local_path = args
                .local_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty());
            let daemon_id = args
                .daemon_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty());
            let (Some(local_path), Some(daemon_id)) = (local_path, daemon_id) else {
                bail!("local_directory requires --local-path and --daemon-id (or pass a JSON payload via --ref)");
            };
            let mut resource_ref = serde_json::Map::from_iter([
                ("local_path".into(), Value::String(local_path.into())),
                ("daemon_id".into(), Value::String(daemon_id.into())),
            ]);
            for (key, value) in [
                ("label", args.ref_label.as_deref()),
                ("execution_mode", args.execution_mode.as_deref()),
            ] {
                if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
                    resource_ref.insert(key.into(), Value::String(value.into()));
                }
            }
            Ok(Value::Object(resource_ref))
        }
        _ => bail!(
            "type {resource_type:?} has no built-in CLI shortcut; pass the payload via --ref '<json>'"
        ),
    }
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

async fn resolve_project_resource_reference(
    client: &ApiClient,
    project_id: &str,
    raw: &str,
) -> Result<(String, String)> {
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok((input.into(), input.into()));
    }
    let Some(prefix) = normalize_uuid_prefix(input) else {
        if input.is_empty() {
            bail!("resolve project resource: project resource id is required");
        }
        let compact = input.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve project resource: expected a full UUID or at least 4 hex characters, got {raw:?}"
            );
        }
        bail!(
            "resolve project resource: expected a UUID prefix containing only hex characters, got {raw:?}"
        );
    };
    let result: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("resolve project resource")?;
    let mut matches = project_resources(&result)
        .iter()
        .filter_map(|resource| {
            let id = value_string(resource, "id");
            if id.is_empty() || !compact_uuid(&id).starts_with(&prefix) {
                return None;
            }
            let label = value_string(resource, "label");
            let resource_type = value_string(resource, "resource_type");
            Some((
                id.clone(),
                if label.is_empty() {
                    if resource_type.is_empty() {
                        id
                    } else {
                        resource_type
                    }
                } else {
                    label
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [(id, display)] => Ok((id.clone(), display.clone())),
        [] => bail!(
            "no project resource found matching id prefix {raw:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous project resource id prefix {raw:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

fn seed_resource_ref(
    existing: Option<&serde_json::Map<String, Value>>,
    keys: &[&str],
) -> serde_json::Map<String, Value> {
    let mut resource_ref = serde_json::Map::new();
    if let Some(existing) = existing {
        for key in keys {
            if let Some(value) = existing
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                resource_ref.insert((*key).into(), Value::String(value.into()));
            }
        }
    }
    resource_ref
}

pub(super) fn build_project_resource_update_ref(
    args: &ProjectResourceUpdateArgs,
    resource_type: &str,
    existing: Option<&serde_json::Map<String, Value>>,
) -> Result<Option<Value>> {
    if let Some(raw) = &args.resource_ref {
        let raw = raw.trim();
        if !raw.is_empty()
            && (resource_type != "github_repo" || raw.starts_with('{') || raw.starts_with('['))
        {
            return parse_generic_resource_ref(raw).map(Some);
        }
        if resource_type != "github_repo" {
            bail!("--ref must be a JSON resource_ref payload for resource type {resource_type:?}");
        }
    }
    match resource_type {
        "github_repo" => {
            if args.url.is_none()
                && args.default_branch_hint.is_none()
                && args.resource_ref.is_none()
            {
                return Ok(None);
            }
            let mut resource_ref =
                seed_resource_ref(existing, &["url", "default_branch_hint", "ref"]);
            if let Some(url) = &args.url {
                let url = url.trim();
                if url.is_empty() {
                    bail!("--url cannot be empty");
                }
                resource_ref.insert("url".into(), Value::String(url.into()));
            }
            for (key, value) in [
                ("default_branch_hint", args.default_branch_hint.as_deref()),
                ("ref", args.resource_ref.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        resource_ref.remove(key);
                    } else {
                        resource_ref.insert(key.into(), Value::String(value.into()));
                    }
                }
            }
            if !resource_ref.contains_key("url") {
                bail!("github_repo: --url is required (no existing url to merge with)");
            }
            Ok(Some(Value::Object(resource_ref)))
        }
        "local_directory" => {
            if args.local_path.is_none()
                && args.daemon_id.is_none()
                && args.ref_label.is_none()
                && args.execution_mode.is_none()
            {
                return Ok(None);
            }
            let mut resource_ref = seed_resource_ref(
                existing,
                &["local_path", "daemon_id", "label", "execution_mode"],
            );
            for (flag, key, value) in [
                ("--local-path", "local_path", args.local_path.as_deref()),
                ("--daemon-id", "daemon_id", args.daemon_id.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        bail!("{flag} cannot be empty");
                    }
                    resource_ref.insert(key.into(), Value::String(value.into()));
                }
            }
            for (key, value) in [
                ("label", args.ref_label.as_deref()),
                ("execution_mode", args.execution_mode.as_deref()),
            ] {
                if let Some(value) = value {
                    let value = value.trim();
                    if value.is_empty() {
                        resource_ref.remove(key);
                    } else {
                        resource_ref.insert(key.into(), Value::String(value.into()));
                    }
                }
            }
            if !resource_ref.contains_key("local_path") {
                bail!("local_directory: --local-path is required (no existing local_path to merge with)");
            }
            if !resource_ref.contains_key("daemon_id") {
                bail!("local_directory: --daemon-id is required (no existing daemon_id to merge with)");
            }
            Ok(Some(Value::Object(resource_ref)))
        }
        _ => {
            if args.url.is_some()
                || args.default_branch_hint.is_some()
                || args.local_path.is_some()
                || args.daemon_id.is_some()
                || args.ref_label.is_some()
                || args.execution_mode.is_some()
            {
                bail!(
                    "no built-in shortcut for resource type {resource_type:?}; pass the full payload via --ref '<json>'"
                );
            }
            Ok(None)
        }
    }
}

fn find_project_resource<'a>(resources: &'a [Value], resource_id: &str) -> Option<&'a Value> {
    resources
        .iter()
        .find(|resource| value_string(resource, "id") == resource_id)
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
