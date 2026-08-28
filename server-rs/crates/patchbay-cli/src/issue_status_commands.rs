use anyhow::{Context, Result};
use serde_json::Value;

use super::{
    new_api_client, resolve_issue_ref, validate_issue_status, value_string, Cli, Environment,
    IssueStatusArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_status(
    cli: &Cli,
    environment: &Environment,
    args: &IssueStatusArgs,
) -> Result<RunOutput> {
    validate_issue_status(&args.status)?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let mut body =
        serde_json::Map::from_iter([("status".into(), Value::String(args.status.clone()))]);
    if args.no_start {
        body.insert("suppress_run".into(), Value::Bool(true));
    }
    let issue: Value = client
        .put_json(&format!("/api/issues/{issue_id}"), &body)
        .await
        .context("update status")?;
    let issue_key = match value_string(&issue, "identifier") {
        value if value.is_empty() => value_string(&issue, "id"),
        value => value,
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput {
        stdout,
        stderr: format!("Issue {issue_key} status changed to {}.\n", args.status),
    })
}
