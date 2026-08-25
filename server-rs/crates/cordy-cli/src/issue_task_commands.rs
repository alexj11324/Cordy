use anyhow::{Context, Result};
use serde_json::Value;
use std::fmt::Write;

use super::issue_task_output::{format_issue_run_messages_table, format_issue_runs_table};
use super::{
    load_issue_actor_names, new_api_client, resolve_current_workspace_id, resolve_issue_ref,
    resolve_task_run_id, value_string, ApiClient, Cli, Environment, IssueCancelTaskArgs,
    IssueRunMessagesArgs, IssueRunsArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_runs(
    cli: &Cli,
    environment: &Environment,
    args: &IssueRunsArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let runs: Vec<Value> = client
        .get_json(&format!("/api/issues/{issue_id}/task-runs"))
        .await
        .context("list runs")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&runs)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let synthetic = runs
                .iter()
                .map(|run| {
                    serde_json::json!({
                        "assignee_type":"agent",
                        "assignee_id":run.get("agent_id").cloned().unwrap_or(Value::Null)
                    })
                })
                .collect::<Vec<_>>();
            let actors = load_issue_actor_names(&client, &workspace_id, &synthetic).await;
            format_issue_runs_table(&runs, args.full_id, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

async fn resolve_task_run_scope(client: &ApiClient, issue: Option<&str>) -> Result<Option<String>> {
    match issue {
        Some(issue) if !issue.is_empty() => Ok(Some(
            resolve_issue_ref(client, issue)
                .await
                .context("resolve issue")?,
        )),
        _ => Ok(None),
    }
}

pub(super) async fn run_issue_run_messages(
    cli: &Cli,
    environment: &Environment,
    args: &IssueRunMessagesArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_task_run_scope(&client, args.issue.as_deref()).await?;
    let task_id = resolve_task_run_id(&client, issue_id.as_deref(), &args.task_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve task run: {error}"))?;
    let mut path = format!("/api/tasks/{task_id}/messages");
    if args.since > 0 {
        let _ = write!(path, "?since={}", args.since);
    }
    let messages: Vec<Value> = client.get_json(&path).await.context("list run messages")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&messages)?),
            OutputFormat::Table => format_issue_run_messages_table(&messages),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_cancel_task(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCancelTaskArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_task_run_scope(&client, args.issue.as_deref()).await?;
    let task_id = resolve_task_run_id(&client, issue_id.as_deref(), &args.task_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve task run: {error}"))?;
    let result: Value = client
        .post_json(
            &format!("/api/tasks/{task_id}/cancel"),
            &serde_json::Map::<String, Value>::new(),
        )
        .await
        .context("cancel task")?;
    let status = match value_string(&result, "status") {
        status if status.is_empty() => "cancelled".into(),
        status => status,
    };
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
            OutputFormat::Table => format!("Task {task_id} -> status={status}\n"),
        },
        stderr: String::new(),
    })
}
