use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

use super::{
    format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, value_string, Cli, Environment, IssueActorNames, OutputFormat, RunOutput,
};

#[derive(Debug, Serialize)]
pub(super) struct IssueChildStageGroup {
    stage: i64,
    total: usize,
    done: usize,
    issues: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct IssueChildrenEnvelope {
    stages: Vec<IssueChildStageGroup>,
    total: usize,
    unstaged: Vec<Value>,
}

pub(super) async fn run_issue_children(
    cli: &Cli,
    environment: &Environment,
    input: &str,
    output: OutputFormat,
    full_id: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, input)
        .await
        .context("resolve issue")?;
    let response: Value = client
        .get_json(&format!("/api/issues/{issue_id}/children"))
        .await
        .context("list child issues")?;
    let mut children = response
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    children.sort_by_key(|child| child_stage(child).map_or((true, 0), |stage| (false, stage)));
    let stdout = match output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string_pretty(&group_issue_children(&children))?
        ),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actors = load_issue_actor_names(&client, &workspace_id, &children).await;
            format_issue_children_table(&children, full_id, &actors)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn child_stage(issue: &Value) -> Option<i64> {
    let value = issue.get("stage")?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
}

fn terminal_child_issue(issue: &Value) -> bool {
    let category = match value_string(issue, "status_category") {
        value if value.is_empty() => value_string(issue, "status"),
        value => value,
    };
    matches!(category.as_str(), "done" | "cancelled")
}

pub(super) fn group_issue_children(children: &[Value]) -> IssueChildrenEnvelope {
    let mut stages = Vec::<IssueChildStageGroup>::new();
    let mut index_by_stage = BTreeMap::<i64, usize>::new();
    let mut unstaged = Vec::new();
    for child in children {
        let Some(stage) = child_stage(child) else {
            unstaged.push(child.clone());
            continue;
        };
        let index = if let Some(index) = index_by_stage.get(&stage) {
            *index
        } else {
            stages.push(IssueChildStageGroup {
                stage,
                total: 0,
                done: 0,
                issues: Vec::new(),
            });
            let index = stages.len() - 1;
            index_by_stage.insert(stage, index);
            index
        };
        let group = &mut stages[index];
        group.total += 1;
        if terminal_child_issue(child) {
            group.done += 1;
        }
        group.issues.push(child.clone());
    }
    IssueChildrenEnvelope {
        stages,
        total: children.len(),
        unstaged,
    }
}

pub(super) fn format_issue_children_table(
    children: &[Value],
    full_id: bool,
    actors: &IssueActorNames,
) -> String {
    let mut rows = Vec::with_capacity(children.len() + 1);
    let mut headers = vec![
        "STAGE".into(),
        "KEY".into(),
        "TITLE".into(),
        "STATUS".into(),
        "PRIORITY".into(),
        "EXECUTOR".into(),
    ];
    if full_id {
        headers.insert(2, "ID".into());
    }
    rows.push(headers);
    rows.extend(children.iter().map(|child| {
        let id = value_string(child, "id");
        let key = match value_string(child, "identifier") {
            value if value.is_empty() => id.clone(),
            value => value,
        };
        let actor_type = value_string(child, "executor_type");
        let actor_id = value_string(child, "executor_id");
        let executor = if actor_type.is_empty() || actor_id.is_empty() {
            String::new()
        } else {
            let actor_key = format!("{actor_type}:{actor_id}");
            actors
                .0
                .get(&actor_key)
                .map_or_else(|| actor_key.clone(), |name| format!("{actor_type}:{name}"))
        };
        let mut row = vec![
            child_stage(child).map_or_else(|| "-".into(), |stage| stage.to_string()),
            key,
            value_string(child, "title"),
            value_string(child, "status"),
            value_string(child, "priority"),
            executor,
        ];
        if full_id {
            row.insert(2, id);
        }
        row
    }));
    format_table(&rows)
}
