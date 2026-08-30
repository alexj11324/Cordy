use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Read;

use super::{
    ensure_file_within_workdir, format_table, new_api_client, resolve_issue_ref, value_string, Cli,
    Environment, IssueDependencyGraphApplyArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_dependency_graph_get(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let graph: Value = client
        .get_json(&format!("/api/issues/{issue_id}/dependency-graph"))
        .await
        .context("get dependency graph")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&graph)?),
            OutputFormat::Table => format_dependency_graph_table(&graph),
        },
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_dependency_graph_apply<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueDependencyGraphApplyArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let idempotency_key = args.idempotency_key.trim();
    if idempotency_key.is_empty() {
        bail!("--idempotency-key must not be empty");
    }
    let plan = read_plan(environment, args, input)?;
    let client = new_api_client(cli, environment)?;
    let parent_id = resolve_issue_ref(&client, &args.parent)
        .await
        .context("resolve parent issue")?;
    let graph: Value = client
        .post_json_with_header(
            &format!("/api/issues/{parent_id}/dependency-graph/apply"),
            &plan,
            "Idempotency-Key",
            idempotency_key,
        )
        .await
        .context("atomically apply dependency graph")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&graph)?),
            OutputFormat::Table => format_dependency_graph_table(&graph),
        },
        stderr: String::new(),
    })
}

fn read_plan<R: Read>(
    environment: &Environment,
    args: &IssueDependencyGraphApplyArgs,
    input: &mut R,
) -> Result<Value> {
    let text = if args.plan_stdin {
        let mut text = String::new();
        input
            .read_to_string(&mut text)
            .context("read typed dependency plan from stdin")?;
        text
    } else {
        let path = args
            .plan_file
            .as_deref()
            .context("one of --plan-file or --plan-stdin is required")?;
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "plan",
        )?;
        fs::read_to_string(path).with_context(|| format!("read dependency plan {:?}", path))?
    };
    let plan: Value = serde_json::from_str(&text).context("parse typed dependency plan JSON")?;
    if !plan.is_object() {
        bail!("typed dependency plan must be a JSON object");
    }
    Ok(plan)
}

pub(super) fn format_dependency_graph_table(graph: &Value) -> String {
    let plan = graph.get("plan").unwrap_or(&Value::Null);
    let nodes = graph
        .get("nodes")
        .and_then(Value::as_array)
        .or_else(|| graph.get("children").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let mut rows = vec![
        vec!["PLAN".into(), value_string(plan, "id")],
        vec!["GOAL".into(), value_string(plan, "goal")],
        vec!["STATUS".into(), value_string(plan, "status")],
    ];
    if let Some(reason) = plan
        .get("attention_reason")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        rows.push(vec!["ATTENTION".into(), reason.to_string()]);
    }
    rows.push(vec!["TASKS".into(), nodes.len().to_string()]);
    rows.push(vec![
        "READY".into(),
        nodes
            .iter()
            .filter(|node| {
                value_string(node.get("readiness").unwrap_or(&Value::Null), "state") == "ready"
            })
            .count()
            .to_string(),
    ]);
    rows.push(vec![
        "BLOCKED".into(),
        nodes
            .iter()
            .filter(|node| {
                value_string(node.get("readiness").unwrap_or(&Value::Null), "state") == "blocked"
            })
            .count()
            .to_string(),
    ]);
    rows.push(vec!["".into(), "".into()]);
    rows.push(vec![
        "TEMP ID".into(),
        "TITLE".into(),
        "STATUS".into(),
        "READINESS".into(),
        "PREREQS".into(),
    ]);
    for node in nodes {
        let readiness = node.get("readiness").unwrap_or(&Value::Null);
        rows.push(vec![
            value_string(&node, "temp_id"),
            value_string(&node, "title"),
            value_string(&node, "status"),
            value_string(readiness, "state"),
            format!(
                "{}/{}",
                value_string(readiness, "satisfied_prerequisites"),
                value_string(readiness, "total_prerequisites")
            ),
        ]);
    }
    format_table(&rows)
}
