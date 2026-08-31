use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;

use super::issue_safety::guard_issue_description_local_links;
use super::{
    format_table, new_api_client, resolve_current_workspace_id, resolve_issue_executor_id,
    resolve_issue_executor_name, resolve_issue_owner_id, resolve_issue_owner_name,
    resolve_issue_project_id, resolve_issue_ref, resolve_issue_reviewer_id,
    resolve_issue_reviewer_name, resolve_issue_update_description, validate_issue_priority,
    validate_issue_status, value_string, Cli, Environment, IssueUpdateArgs, OutputFormat,
    RunOutput,
};

pub(super) async fn run_issue_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueUpdateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    if let Some(status) = &args.status {
        validate_issue_status(status)?;
    }
    if let Some(priority) = &args.priority {
        validate_issue_priority(priority)?;
    }
    if args.executor.is_some() && args.executor_id.is_some() {
        bail!("--executor and --executor-id are mutually exclusive");
    }
    if args.owner.is_some() && args.owner_id.is_some() {
        bail!("--owner and --owner-id are mutually exclusive");
    }
    if args.reviewer.is_some() && args.reviewer_id.is_some() {
        bail!("--reviewer and --reviewer-id are mutually exclusive");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let mut body = serde_json::Map::new();
    if let Some(title) = &args.title {
        body.insert("title".into(), Value::String(title.clone()));
    }
    if args.description.is_some() || args.description_stdin || args.description_file.is_some() {
        let description = resolve_issue_update_description(args, environment, input)?;
        guard_issue_description_local_links(
            &description,
            environment,
            "`patchbay issue update` cannot carry files — deliver the file with `patchbay issue comment add <issue-id> --attachment <path>` instead, and drop the link.",
        )?;
        body.insert("description".into(), Value::String(description));
    }
    if let Some(status) = &args.status {
        body.insert("status".into(), Value::String(status.clone()));
    }
    if let Some(priority) = &args.priority {
        body.insert("priority".into(), Value::String(priority.clone()));
    }
    if let Some(project) = &args.project {
        if project.is_empty() {
            body.insert("project_id".into(), Value::Null);
        } else {
            let project_id = resolve_issue_project_id(&client, &workspace_id, project)
                .await
                .context("resolve project")?;
            body.insert("project_id".into(), Value::String(project_id));
        }
    }
    if let Some(start_date) = &args.start_date {
        body.insert("start_date".into(), Value::String(start_date.clone()));
    }
    if let Some(due_date) = &args.due_date {
        body.insert("due_date".into(), Value::String(due_date.clone()));
    }
    let executor = if let Some(id) = &args.executor_id {
        Some(
            resolve_issue_executor_id(&client, &workspace_id, id)
                .await
                .context("resolve executor")?,
        )
    } else if let Some(name) = &args.executor {
        Some(
            resolve_issue_executor_name(&client, &workspace_id, name)
                .await
                .context("resolve executor")?,
        )
    } else {
        None
    };
    if let Some(executor) = executor {
        if !matches!(executor.actor_type.as_str(), "agent" | "team") {
            bail!("--executor resolves to a member; use --owner for human ownership");
        }
        body.insert("executor_type".into(), Value::String(executor.actor_type));
        body.insert("executor_id".into(), Value::String(executor.id));
    }
    let owner = if let Some(id) = &args.owner_id {
        Some(
            resolve_issue_owner_id(&client, &workspace_id, id)
                .await
                .context("resolve owner")?,
        )
    } else if let Some(name) = &args.owner {
        Some(
            resolve_issue_owner_name(&client, &workspace_id, name)
                .await
                .context("resolve owner")?,
        )
    } else {
        None
    };
    if let Some(owner) = owner {
        body.insert("owner_type".into(), Value::String(owner.actor_type));
        body.insert("owner_id".into(), Value::String(owner.id));
    }
    let reviewer = if let Some(id) = &args.reviewer_id {
        Some(
            resolve_issue_reviewer_id(&client, &workspace_id, id)
                .await
                .context("resolve reviewer")?,
        )
    } else if let Some(name) = &args.reviewer {
        Some(
            resolve_issue_reviewer_name(&client, &workspace_id, name)
                .await
                .context("resolve reviewer")?,
        )
    } else {
        None
    };
    if let Some(reviewer) = reviewer {
        body.insert("reviewer_type".into(), Value::String(reviewer.actor_type));
        body.insert("reviewer_id".into(), Value::String(reviewer.id));
    }
    if let Some(parent) = &args.parent {
        if parent.is_empty() {
            body.insert("parent_issue_id".into(), Value::Null);
        } else {
            let parent_id = resolve_issue_ref(&client, parent)
                .await
                .context("resolve parent issue")?;
            body.insert("parent_issue_id".into(), Value::String(parent_id));
        }
    }
    if let Some(stage) = args.stage {
        if stage < 1 {
            bail!("--stage must be >= 1");
        }
        body.insert("stage".into(), Value::Number(stage.into()));
    }
    if let Some(position) = args.position {
        let position =
            serde_json::Number::from_f64(position).context("--position must be a finite number")?;
        body.insert("position".into(), Value::Number(position));
    }
    if body.is_empty() {
        bail!(
            "no fields to update; use flags like --title, --status, --priority, --executor, etc."
        );
    }
    if args.no_start {
        body.insert("suppress_run".into(), Value::Bool(true));
    }

    let issue: Value = client
        .put_json(&format!("/api/issues/{issue_id}"), &body)
        .await
        .context("update issue")?;
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => value_string(&issue, "id"),
        value => value,
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => format_table(&[
            vec![
                "KEY".into(),
                "TITLE".into(),
                "STATUS".into(),
                "PRIORITY".into(),
            ],
            vec![
                issue_key,
                value_string(&issue, "title"),
                value_string(&issue, "status"),
                value_string(&issue, "priority"),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
