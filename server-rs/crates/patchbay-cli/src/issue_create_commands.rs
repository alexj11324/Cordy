use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt::Write;
use std::io::Read;

use super::issue_safety::{active_duplicate_issue_message, guard_issue_description_local_links};
use super::{
    append_unique_strings, collect_local_attachments, format_table, http_timeout, new_api_client,
    quick_create_attachment_ids, resolve_current_workspace_id, resolve_issue_assignee_id,
    resolve_issue_assignee_name, resolve_issue_create_description, resolve_issue_project_id,
    resolve_issue_ref, validate_issue_priority, validate_issue_status, value_string, Cli,
    Environment, IssueCreateArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_create<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCreateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let title = args.title.as_deref().unwrap_or_default();
    if title.is_empty() {
        bail!("--title is required");
    }
    if let Some(status) = args.status.as_deref().filter(|value| !value.is_empty()) {
        validate_issue_status(status)?;
    }
    if let Some(priority) = args.priority.as_deref().filter(|value| !value.is_empty()) {
        validate_issue_priority(priority)?;
    }

    let mut client = new_api_client(cli, environment)?;
    if !args.attachment.is_empty() {
        let timeout = http_timeout(environment.raw("PATCHBAY_HTTP_TIMEOUT"))
            .max(std::time::Duration::from_secs(60));
        client = client.with_request_timeout(timeout);
    }

    let mut body = serde_json::Map::new();
    body.insert("title".into(), Value::String(title.into()));
    if let Some(description) = resolve_issue_create_description(args, environment, input)? {
        guard_issue_description_local_links(
            &description,
            environment,
            "Deliver the file itself with `patchbay issue create --attachment <path>` (repeatable) and drop the link.",
        )?;
        body.insert("description".into(), Value::String(description));
    }
    if let Some(status) = args.status.as_deref().filter(|value| !value.is_empty()) {
        body.insert("status".into(), Value::String(status.into()));
    }
    if let Some(priority) = args.priority.as_deref().filter(|value| !value.is_empty()) {
        body.insert("priority".into(), Value::String(priority.into()));
    }
    if let Some(parent) = args.parent.as_deref().filter(|value| !value.is_empty()) {
        let parent_id = resolve_issue_ref(&client, parent)
            .await
            .context("resolve parent issue")?;
        body.insert("parent_issue_id".into(), Value::String(parent_id));
    }
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if let Some(project) = args.project.as_deref().filter(|value| !value.is_empty()) {
        let project_id = resolve_issue_project_id(&client, &workspace_id, project)
            .await
            .context("resolve project")?;
        body.insert("project_id".into(), Value::String(project_id));
    }
    if let Some(stage) = args.stage {
        if stage < 1 {
            bail!("--stage must be >= 1");
        }
        body.insert("stage".into(), Value::Number(stage.into()));
    }
    if let Some(start_date) = args.start_date.as_deref().filter(|value| !value.is_empty()) {
        body.insert("start_date".into(), Value::String(start_date.into()));
    }
    if let Some(due_date) = args.due_date.as_deref().filter(|value| !value.is_empty()) {
        body.insert("due_date".into(), Value::String(due_date.into()));
    }
    if args.allow_duplicate {
        body.insert("allow_duplicate".into(), Value::Bool(true));
    }
    if args.assignee.is_some() && args.assignee_id.is_some() {
        bail!("--assignee and --assignee-id are mutually exclusive");
    }
    let assignee = if let Some(id) = &args.assignee_id {
        Some(
            resolve_issue_assignee_id(&client, &workspace_id, id)
                .await
                .context("resolve assignee")?,
        )
    } else if let Some(name) = &args.assignee {
        Some(
            resolve_issue_assignee_name(&client, &workspace_id, name)
                .await
                .context("resolve assignee")?,
        )
    } else {
        None
    };
    if let Some(assignee) = assignee {
        body.insert("assignee_type".into(), Value::String(assignee.actor_type));
        body.insert("assignee_id".into(), Value::String(assignee.id));
    }
    if let Some(task_id) = environment
        .raw("PATCHBAY_QUICK_CREATE_TASK_ID")
        .filter(|value| !value.is_empty())
    {
        body.insert("origin_type".into(), Value::String("quick_create".into()));
        body.insert("origin_id".into(), Value::String(task_id.into()));
    }
    let mut attachment_ids = append_unique_strings(args.attachment_id.iter().cloned());
    let env_attachment_ids = quick_create_attachment_ids(environment)?;
    attachment_ids = append_unique_strings(attachment_ids.into_iter().chain(env_attachment_ids));
    if !attachment_ids.is_empty() {
        body.insert(
            "attachment_ids".into(),
            Value::Array(attachment_ids.into_iter().map(Value::String).collect()),
        );
    }

    let (pending, mut stderr) =
        collect_local_attachments(&args.attachment, args.allow_external_file, environment)?;
    let issue: Value = match client.post_json("/api/issues", &body).await {
        Ok(issue) => issue,
        Err(error) => {
            if let Some(message) = active_duplicate_issue_message(&error) {
                bail!("{message}");
            }
            return Err(error).context("create issue");
        }
    };
    let issue_id = value_string(&issue, "id");
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => issue_id.clone(),
        value => value,
    };
    for attachment in pending {
        match client
            .upload_file(attachment.data, &attachment.path, &issue_id)
            .await
        {
            Ok(_) => {
                let _ = writeln!(stderr, "Uploaded {}", attachment.path);
            }
            Err(error) => {
                let _ = writeln!(
                    stderr,
                    "warning: upload attachment {} failed (issue already created, {}): {}",
                    attachment.path, issue_key, error
                );
            }
        }
    }
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
    Ok(RunOutput { stdout, stderr })
}
