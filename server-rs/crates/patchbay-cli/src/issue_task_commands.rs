use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt::Write;
use std::io::Read;

use super::{
    display_id, format_metadata_value, format_table, load_issue_actor_names, new_api_client,
    resolve_current_workspace_id, resolve_issue_ref, resolve_task_run_id, value_string, ApiClient,
    trim_one_trailing_newline, unescape_backslash_escapes, Cli, Environment, IssueActorNames,
    IssueCancelTaskArgs, IssueMessageMainArgs, IssueRunMessagesArgs, IssueRunsArgs, OutputFormat,
    RunOutput,
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

pub(super) fn format_issue_runs_table(
    runs: &[Value],
    full_id: bool,
    actors: &IssueActorNames,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "AGENT".into(),
        "STATUS".into(),
        "STARTED".into(),
        "COMPLETED".into(),
        "ERROR".into(),
    ]];
    for run in runs {
        let agent_id = value_string(run, "agent_id");
        let agent = actors
            .0
            .get(&format!("agent:{agent_id}"))
            .cloned()
            .unwrap_or(agent_id);
        let error = value_string(run, "error");
        let error = if error.chars().count() > 50 {
            format!("{}...", error.chars().take(47).collect::<String>())
        } else {
            error
        };
        let timestamp = |field| {
            value_string(run, field)
                .chars()
                .take(16)
                .collect::<String>()
        };
        rows.push(vec![
            display_id(&value_string(run, "id"), full_id),
            agent,
            value_string(run, "status"),
            timestamp("started_at"),
            timestamp("completed_at"),
            error,
        ]);
    }
    format_table(&rows)
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

pub(super) async fn run_issue_message_main<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMessageMainArgs,
    input: &mut R,
) -> Result<RunOutput> {
    if args.content_stdin && args.content.is_some() {
        bail!("--content and --content-stdin are mutually exclusive");
    }
    let content = if args.content_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --content-stdin")?;
        trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        args.content
            .as_deref()
            .map(unescape_backslash_escapes)
            .unwrap_or_default()
    };
    if content.trim().is_empty() {
        bail!("--content or --content-stdin is required");
    }

    let client = new_api_client(cli, environment)?;
    let task_id = resolve_task_run_id(&client, None, &args.task_id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve main task run: {error}"))?;
    let result: Value = client
        .post_json(
            &format!("/api/tasks/{task_id}/message-bus"),
            &serde_json::json!({ "content": content }),
        )
        .await
        .context("send Side Chat instruction to the main task")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => format!(
            "Instruction {} for main task {}.\n",
            value_string(&result, "status"),
            value_string(&result, "main_task_id")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_issue_run_messages_table(messages: &[Value]) -> String {
    let mut rows = vec![vec![
        "SEQ".into(),
        "TYPE".into(),
        "TOOL".into(),
        "CONTENT".into(),
    ]];
    for message in messages {
        let mut content = value_string(message, "content");
        if content.is_empty() {
            content = value_string(message, "output");
        }
        if content.chars().count() > 80 {
            content = format!("{}...", content.chars().take(77).collect::<String>());
        }
        rows.push(vec![
            message
                .get("seq")
                .map(|value| format_metadata_value(Some(value)))
                .unwrap_or_default(),
            value_string(message, "type"),
            value_string(message, "tool"),
            content,
        ]);
    }
    format_table(&rows)
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
