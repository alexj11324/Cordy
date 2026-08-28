use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use url::form_urlencoded;

use super::{
    format_table, new_api_client, resolve_current_workspace_id, resolve_issue_ref, value_string,
    ApiClient, Cli, Environment, IssueReorderArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_reorder(
    cli: &Cli,
    environment: &Environment,
    args: &IssueReorderArgs,
) -> Result<RunOutput> {
    if args.before.as_deref() == Some("") {
        bail!("--before requires an issue ID or key");
    }
    if args.after.as_deref() == Some("") {
        bail!("--after requires an issue ID or key");
    }
    if args.top == Some(false) {
        bail!("--top cannot be set to false; pass it on its own to move the issue to the top of its column");
    }
    if args.bottom == Some(false) {
        bail!("--bottom cannot be set to false; pass it on its own to move the issue to the bottom of its column");
    }

    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if workspace_id.is_empty() {
        bail!("no workspace configured; pass --workspace-id, set PATCHBAY_WORKSPACE_ID, or configure a default workspace");
    }
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let target: Value = client
        .get_json(&format!("/api/issues/{issue_id}"))
        .await
        .context("get issue")?;
    let issue_key = issue_value_key(&target);
    let status = value_string(&target, "status");
    if status.is_empty() {
        bail!("issue {issue_key} has no status, cannot determine its column");
    }

    let relative_input = args.before.as_deref().or(args.after.as_deref());
    let other = if let Some(input) = relative_input {
        let id = resolve_issue_ref(&client, input)
            .await
            .context("resolve target issue")?;
        if id == issue_id {
            bail!("cannot reorder issue {issue_key} relative to itself");
        }
        Some((id, input.to_string()))
    } else {
        None
    };

    let project_id = value_string(&target, "project_id");
    let column = fetch_issue_column(&client, &workspace_id, &project_id, &status).await?;
    let mut positions = HashMap::with_capacity(column.len());
    let mut ordered = Vec::with_capacity(column.len());
    for issue in &column {
        let id = value_string(issue, "id");
        if id.is_empty() {
            continue;
        }
        positions.insert(
            id.clone(),
            issue.get("position").and_then(Value::as_f64).unwrap_or(0.0),
        );
        if id != issue_id {
            ordered.push(id);
        }
    }
    if ordered.is_empty() {
        if let Some((other_id, other_display)) = &other {
            return Err(reorder_target_not_in_column(
                &client,
                other_id,
                other_display,
                &issue_key,
                &status,
            )
            .await);
        }
        return issue_reorder_output(
            &target,
            args.output,
            format!(
                "Issue {issue_key} is the only issue in the {status} column; nothing to reorder.\n"
            ),
        );
    }

    let insert_index = if args.top == Some(true) {
        0
    } else if args.bottom == Some(true) {
        ordered.len()
    } else {
        let Some((other_id, other_display)) = other.as_ref() else {
            bail!("exactly one of --before, --after, --top, or --bottom is required");
        };
        let Some(index) = ordered.iter().position(|id| id == other_id) else {
            return Err(reorder_target_not_in_column(
                &client,
                other_id,
                other_display,
                &issue_key,
                &status,
            )
            .await);
        };
        index + usize::from(args.after.is_some())
    };
    let mut reordered = Vec::with_capacity(ordered.len() + 1);
    reordered.extend_from_slice(&ordered[..insert_index]);
    reordered.push(issue_id.clone());
    reordered.extend_from_slice(&ordered[insert_index..]);
    let current_position = positions.get(&issue_id).copied().unwrap_or(0.0);
    let new_position =
        compute_reorder_position(&reordered, &issue_id, &positions, current_position);
    if new_position == current_position {
        return issue_reorder_output(
            &target,
            args.output,
            format!("Issue {issue_key} is already in that position.\n"),
        );
    }

    let issue: Value = client
        .put_json(
            &format!("/api/issues/{issue_id}"),
            &serde_json::json!({"position": new_position}),
        )
        .await
        .context("reorder issue")?;
    let result_key = issue_value_key(&issue);
    issue_reorder_output(
        &issue,
        args.output,
        format!("Issue {result_key} reordered.\n"),
    )
}

fn issue_value_key(issue: &Value) -> String {
    match value_string(issue, "identifier") {
        value if value.is_empty() => value_string(issue, "id"),
        value => value,
    }
}

fn issue_reorder_output(issue: &Value, output: OutputFormat, stderr: String) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(issue)?),
        OutputFormat::Table => format_table(&[
            vec![
                "KEY".into(),
                "TITLE".into(),
                "STATUS".into(),
                "PRIORITY".into(),
            ],
            vec![
                issue_value_key(issue),
                value_string(issue, "title"),
                value_string(issue, "status"),
                value_string(issue, "priority"),
            ],
        ]),
    };
    Ok(RunOutput { stdout, stderr })
}

async fn fetch_issue_column(
    client: &ApiClient,
    workspace_id: &str,
    project_id: &str,
    status: &str,
) -> Result<Vec<Value>> {
    let mut issues = Vec::new();
    let mut offset = 0_i64;
    loop {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("workspace_id", workspace_id);
        serializer.append_pair("status", status);
        if !project_id.is_empty() {
            serializer.append_pair("project_id", project_id);
        }
        serializer.append_pair("sort", "position");
        serializer.append_pair("limit", "100");
        serializer.append_pair("offset", &offset.to_string());
        let result: Value = client
            .get_json(&format!("/api/issues?{}", serializer.finish()))
            .await
            .with_context(|| format!("list {status} column"))?;
        let page = result
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_len = page.len() as i64;
        issues.extend(page);
        offset += page_len;
        let total = result.get("total").and_then(Value::as_i64).unwrap_or(0);
        if page_len == 0 || offset >= total {
            break;
        }
    }
    Ok(issues)
}

async fn reorder_target_not_in_column(
    client: &ApiClient,
    other_id: &str,
    other_display: &str,
    issue_display: &str,
    status: &str,
) -> anyhow::Error {
    if let Ok(other) = client
        .get_json::<Value>(&format!("/api/issues/{other_id}"))
        .await
    {
        let other_status = value_string(&other, "status");
        if !other_status.is_empty() && other_status != status {
            return anyhow::anyhow!(
                "issue {other_display} is in the {other_status:?} column but {issue_display} is in {status:?}; move one with `patchbay issue status` first, or pick a target in the same column"
            );
        }
    }
    anyhow::anyhow!("issue {other_display} was not found in the {status:?} column")
}

pub(super) fn compute_reorder_position(
    ids: &[String],
    active_id: &str,
    positions: &HashMap<String, f64>,
    fallback: f64,
) -> f64 {
    let Some(index) = ids.iter().position(|id| id == active_id) else {
        return fallback;
    };
    if ids.len() == 1 {
        fallback
    } else if index == 0 {
        positions.get(&ids[1]).copied().unwrap_or(0.0) - 1.0
    } else if index == ids.len() - 1 {
        positions.get(&ids[index - 1]).copied().unwrap_or(0.0) + 1.0
    } else {
        (positions.get(&ids[index - 1]).copied().unwrap_or(0.0)
            + positions.get(&ids[index + 1]).copied().unwrap_or(0.0))
            / 2.0
    }
}
