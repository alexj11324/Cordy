use anyhow::{Context, Result};
use serde_json::Value;

use super::issue_timeline_filter::{build_timeline_filter, filter_timeline};
use super::issue_timeline_output::format_issue_timeline_table;
use super::{
    load_issue_actor_names, new_api_client, resolve_current_workspace_id, resolve_issue_ref, Cli,
    Environment, IssueTimelineArgs, OutputFormat, RunOutput,
};

fn timeline_actor_inputs(entries: &[Value]) -> Vec<Value> {
    let mut actors = Vec::new();
    for entry in entries {
        actors.push(serde_json::json!({
            "assignee_type":entry.get("actor_type").cloned().unwrap_or(Value::Null),
            "assignee_id":entry.get("actor_id").cloned().unwrap_or(Value::Null),
        }));
        if let Some(details) = entry.get("details") {
            for prefix in ["from", "to"] {
                actors.push(serde_json::json!({
                    "assignee_type":details.get(format!("{prefix}_type")).cloned().unwrap_or(Value::Null),
                    "assignee_id":details.get(format!("{prefix}_id")).cloned().unwrap_or(Value::Null),
                }));
            }
        }
    }
    actors
}

pub(super) async fn run_issue_timeline(
    cli: &Cli,
    environment: &Environment,
    args: &IssueTimelineArgs,
) -> Result<RunOutput> {
    let filter = build_timeline_filter(args)?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let (entries, headers) = client
        .get_json_with_headers::<Vec<Value>>(&format!("/api/issues/{issue_id}/timeline"))
        .await
        .context("list issue timeline")?;
    let entries = filter_timeline(entries, &filter);
    let truncated = headers
        .get("X-Timeline-Truncated")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let stderr = if truncated.is_empty() {
        String::new()
    } else {
        format!(
            "warning: timeline truncated by the server cap ({truncated}): older entries are missing. Durations and \"first entered <status>\" cannot be concluded from this read.\n"
        )
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&entries)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actor_inputs = timeline_actor_inputs(&entries);
            let actors = load_issue_actor_names(&client, &workspace_id, &actor_inputs).await;
            format_issue_timeline_table(&entries, &actors, args.full_id)
        }
    };
    Ok(RunOutput { stdout, stderr })
}
