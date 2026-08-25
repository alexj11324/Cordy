use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use url::form_urlencoded;

use super::{
    format_issue_list_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_assignee_id, resolve_issue_assignee_name, resolve_issue_project_id, ApiClient,
    Cli, Environment, IssueListArgs, IssueListResponse, OutputFormat, RunOutput,
    VALID_ISSUE_SORT_COLUMNS,
};

#[derive(Debug, Serialize)]
struct IssueListEnvelope<'a> {
    has_more: bool,
    issues: &'a [Value],
    limit: i64,
    offset: i64,
    total: i64,
}

pub(super) async fn run_issue_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if workspace_id.is_empty() {
        if environment.in_daemon_managed_execution_context() {
            bail!(
                "workspace_id is required: CORDY_WORKSPACE_ID must be set by the daemon in agent execution context (no fallback to user config)"
            );
        }
        bail!(
            "workspace_id is required: use --workspace-id flag, set CORDY_WORKSPACE_ID env, or run 'cordy config set workspace_id <id>'"
        );
    }

    let query = build_issue_list_query(&client, &workspace_id, args).await?;
    let path = format!("/api/issues?{query}");
    let result: IssueListResponse = client.get_json(&path).await.context("list issues")?;
    let issues = result.issues.as_array().cloned().unwrap_or_default();
    let total = result.total.as_f64().unwrap_or_default() as i64;

    let stdout = match args.output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&IssueListEnvelope {
                has_more: issue_list_has_more(args.offset, issues.len(), total),
                issues: &issues,
                limit: args.limit,
                offset: args.offset,
                total,
            })?
        ),
        OutputFormat::Table => {
            let actors = load_issue_actor_names(&client, &workspace_id, &issues).await;
            format_issue_list_table(&issues, args.full_id, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn issue_list_has_more(offset: i64, issue_count: usize, total: i64) -> bool {
    offset + (issue_count as i64) < total
}

pub(super) async fn build_issue_list_query(
    client: &ApiClient,
    workspace_id: &str,
    args: &IssueListArgs,
) -> Result<String> {
    let mut params = BTreeMap::<String, String>::new();
    params.insert("workspace_id".into(), workspace_id.into());
    if let Some(status) = args.status.as_deref().filter(|value| !value.is_empty()) {
        params.insert("status".into(), status.into());
    }
    if let Some(priority) = args.priority.as_deref().filter(|value| !value.is_empty()) {
        params.insert("priority".into(), priority.into());
    }
    if args.limit > 0 {
        params.insert("limit".into(), args.limit.to_string());
    }
    if args.offset > 0 {
        params.insert("offset".into(), args.offset.to_string());
    }

    if args.assignee.is_some() && args.assignee_id.is_some() {
        bail!("--assignee and --assignee-id are mutually exclusive");
    }
    if let Some(id) = &args.assignee_id {
        let assignee = resolve_issue_assignee_id(client, workspace_id, id)
            .await
            .context("resolve assignee")?;
        params.insert("assignee_id".into(), assignee.id);
    } else if let Some(name) = &args.assignee {
        let assignee = resolve_issue_assignee_name(client, workspace_id, name)
            .await
            .context("resolve assignee")?;
        params.insert("assignee_id".into(), assignee.id);
    }

    if let Some(project) = args.project.as_deref().filter(|value| !value.is_empty()) {
        params.insert(
            "project_id".into(),
            resolve_issue_project_id(client, workspace_id, project).await?,
        );
    }
    if !args.metadata.is_empty() {
        params.insert("metadata".into(), build_metadata_filter(&args.metadata)?);
    }
    if let Some(sort) = args.sort.as_deref().filter(|value| !value.is_empty()) {
        if !VALID_ISSUE_SORT_COLUMNS.contains(&sort) {
            bail!(
                "invalid --sort {sort:?}; valid values: {}",
                VALID_ISSUE_SORT_COLUMNS.join(", ")
            );
        }
        params.insert("sort".into(), sort.into());
    }
    if let Some(direction) = args.direction.as_deref().filter(|value| !value.is_empty()) {
        let direction = direction.to_ascii_lowercase();
        if direction != "asc" && direction != "desc" {
            bail!(
                "invalid --direction {:?}; valid values: asc, desc",
                args.direction.as_deref().unwrap_or_default()
            );
        }
        if matches!(args.sort.as_deref(), None | Some("") | Some("position")) {
            bail!(
                "--direction requires --sort to be one of title, created_at, start_date, due_date, priority; position (the default manual board order) is always ascending"
            );
        }
        params.insert("direction".into(), direction);
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(&key, &value);
    }
    Ok(serializer.finish())
}

pub(super) fn build_metadata_filter(pairs: &[String]) -> Result<String> {
    let mut values = BTreeMap::<String, Value>::new();
    for pair in pairs {
        let Some((key, raw)) = pair.split_once('=') else {
            bail!("--metadata {pair:?} must be in key=value form");
        };
        if key.is_empty() {
            bail!("--metadata {pair:?} must be in key=value form");
        }
        if values.contains_key(key) {
            bail!("--metadata key {key:?} given more than once; combine into a single filter");
        }
        let parsed = serde_json::from_str::<Value>(raw).ok();
        let value = match parsed {
            Some(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => value,
            _ => Value::String(raw.into()),
        };
        values.insert(key.into(), value);
    }
    serde_json::to_string(&values).context("encode metadata filter")
}
