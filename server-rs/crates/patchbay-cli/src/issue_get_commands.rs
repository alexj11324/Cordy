use anyhow::{Context, Result};
use serde_json::Value;

use super::{
    format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, value_string, Cli, Environment, IssueActorNames, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_get(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let issue: Value = client
        .get_json(&format!("/api/issues/{issue_id}"))
        .await
        .context("get issue")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors =
                load_issue_actor_names(&client, &workspace_id, std::slice::from_ref(&issue)).await;
            format_issue_get_table(&issue, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_issue_get_table(issue: &Value, actors: &IssueActorNames) -> String {
    let id = value_string(issue, "id");
    let key = match value_string(issue, "identifier") {
        value if value.is_empty() => id,
        value => value,
    };
    let actor_type = value_string(issue, "assignee_type");
    let actor_id = value_string(issue, "assignee_id");
    let assignee = if actor_type.is_empty() || actor_id.is_empty() {
        String::new()
    } else {
        let actor_key = format!("{actor_type}:{actor_id}");
        actors
            .0
            .get(&actor_key)
            .map_or_else(|| actor_key.clone(), |name| format!("{actor_type}:{name}"))
    };
    let date = |field| {
        value_string(issue, field)
            .chars()
            .take(10)
            .collect::<String>()
    };
    format_table(&[
        vec![
            "KEY".into(),
            "TITLE".into(),
            "STATUS".into(),
            "PRIORITY".into(),
            "ASSIGNEE".into(),
            "START DATE".into(),
            "DUE DATE".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            key,
            value_string(issue, "title"),
            value_string(issue, "status"),
            value_string(issue, "priority"),
            assignee,
            date("start_date"),
            date("due_date"),
            value_string(issue, "description"),
        ],
    ])
}
