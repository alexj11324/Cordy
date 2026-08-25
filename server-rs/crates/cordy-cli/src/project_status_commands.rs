use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    new_api_client, resolve_current_workspace_id, resolve_issue_project_id, value_string, Cli,
    Environment, OutputFormat, RunOutput,
};

pub(super) const PROJECT_STATUSES: &[&str] =
    &["planned", "in_progress", "paused", "completed", "cancelled"];

pub(super) fn validate_project_status(status: &str) -> Result<()> {
    if PROJECT_STATUSES.contains(&status) {
        Ok(())
    } else {
        bail!(
            "invalid status {status:?}; valid values: {}",
            PROJECT_STATUSES.join(", ")
        )
    }
}

pub(super) async fn run_project_status(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    status: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    validate_project_status(status)?;
    let client = new_api_client(cli, environment)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let project_id = resolve_issue_project_id(&client, &workspace_id, id)
        .await
        .map_err(|error| anyhow::anyhow!("resolve project: {error}"))?;
    let project: Value = client
        .put_json(
            &format!("/api/projects/{project_id}"),
            &serde_json::json!({"status":status}),
        )
        .await
        .context("update status")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&project)?),
            OutputFormat::Table => String::new(),
        },
        stderr: format!(
            "Project {} status changed to {status}.\n",
            value_string(&project, "title")
        ),
    })
}
