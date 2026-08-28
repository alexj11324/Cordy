use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    new_api_client, resolve_current_workspace_id, resolve_issue_assignee_id,
    resolve_issue_assignee_name, resolve_issue_ref, value_string, Cli, Environment,
    IssueAssignArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_assign(
    cli: &Cli,
    environment: &Environment,
    args: &IssueAssignArgs,
) -> Result<RunOutput> {
    if args.to.is_none() && args.to_id.is_none() && !args.unassign {
        bail!("provide --to <name>, --to-id <uuid>, or --unassign");
    }
    if (args.to.is_some() || args.to_id.is_some()) && args.unassign {
        bail!("--to/--to-id and --unassign are mutually exclusive");
    }
    if args.to.is_some() && args.to_id.is_some() {
        bail!("--to and --to-id are mutually exclusive");
    }
    if args.no_start && args.unassign {
        bail!("--no-start cannot be used with --unassign");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let mut body = serde_json::Map::new();
    let display_target = if args.unassign {
        body.insert("assignee_type".into(), Value::Null);
        body.insert("assignee_id".into(), Value::Null);
        None
    } else {
        let workspace_id = resolve_current_workspace_id(cli, environment);
        let assignee = if let Some(id) = &args.to_id {
            resolve_issue_assignee_id(&client, &workspace_id, id)
                .await
                .context("resolve assignee")?
        } else {
            resolve_issue_assignee_name(
                &client,
                &workspace_id,
                args.to.as_deref().unwrap_or_default(),
            )
            .await
            .context("resolve assignee")?
        };
        let display = args.to.clone().unwrap_or_else(|| {
            if assignee.name.is_empty() {
                format!("{}:{}", assignee.actor_type, assignee.id)
            } else {
                format!("{}:{}", assignee.actor_type, assignee.name)
            }
        });
        body.insert("assignee_type".into(), Value::String(assignee.actor_type));
        body.insert("assignee_id".into(), Value::String(assignee.id));
        if args.no_start {
            body.insert("suppress_run".into(), Value::Bool(true));
        }
        Some(display)
    };

    let issue: Value = client
        .put_json(&format!("/api/issues/{issue_id}"), &body)
        .await
        .context("assign issue")?;
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => value_string(&issue, "id"),
        value => value,
    };
    let stderr = if let Some(target) = display_target {
        format!("Issue {issue_key} assigned to {target}.\n")
    } else {
        format!("Issue {issue_key} unassigned.\n")
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput { stdout, stderr })
}
