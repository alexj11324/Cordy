use anyhow::{Context, Result};
use serde_json::Value;

use super::{
    format_label_table, issue_labels, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, resolve_label_id, ApiClient, Cli, Environment, IssueLabelListArgs,
    IssueLabelMutationArgs, OutputFormat, RunOutput,
};

pub(super) fn format_issue_labels(
    labels: &[Value],
    output: OutputFormat,
    full_id: bool,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(labels)?)),
        OutputFormat::Table => Ok(format_label_table(labels, full_id)),
    }
}

pub(super) async fn run_issue_label_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .get_json(&format!("/api/issues/{issue_id}/labels"))
        .await
        .context("list issue labels")?;
    Ok(RunOutput {
        stdout: format_issue_labels(issue_labels(&result), args.output, args.full_id)?,
        stderr: String::new(),
    })
}

async fn resolve_issue_and_label(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelMutationArgs,
) -> Result<(ApiClient, String, String)> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let label_id = resolve_label_id(&client, &workspace_id, &args.label_id)
        .await
        .context("resolve label")?;
    Ok((client, issue_id, label_id))
}

pub(super) async fn run_issue_label_add(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelMutationArgs,
) -> Result<RunOutput> {
    let (client, issue_id, label_id) = resolve_issue_and_label(cli, environment, args).await?;
    let result: Value = client
        .post_json(
            &format!("/api/issues/{issue_id}/labels"),
            &serde_json::json!({"label_id":label_id}),
        )
        .await
        .context("attach label")?;
    Ok(RunOutput {
        stdout: format_issue_labels(issue_labels(&result), args.output, args.full_id)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_label_remove(
    cli: &Cli,
    environment: &Environment,
    args: &IssueLabelMutationArgs,
) -> Result<RunOutput> {
    let (client, issue_id, label_id) = resolve_issue_and_label(cli, environment, args).await?;
    client
        .delete(&format!("/api/issues/{issue_id}/labels/{label_id}"))
        .await
        .context("detach label")?;
    let result = client
        .get_json::<Value>(&format!("/api/issues/{issue_id}/labels"))
        .await;
    let stdout = match result {
        Ok(result) => format_issue_labels(issue_labels(&result), args.output, args.full_id)?,
        Err(_) if args.output == OutputFormat::Json => "{\n  \"detached\": true\n}\n".into(),
        Err(_) => "Label detached.\n".into(),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
