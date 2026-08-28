use anyhow::{Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{
    new_api_client, Cli, Environment, IssueCommentResolutionArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_comment_delete(
    cli: &Cli,
    environment: &Environment,
    comment_id: &str,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!("/api/comments/{comment_id}"))
        .await
        .context("delete comment")?;
    Ok(RunOutput {
        stdout: String::new(),
        stderr: format!("Comment {comment_id} deleted.\n"),
    })
}

pub(super) async fn run_issue_comment_resolution(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCommentResolutionArgs,
    resolve: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let comment_id = args.comment_id.trim();
    let encoded_id = form_urlencoded::byte_serialize(comment_id.as_bytes()).collect::<String>();
    let path = format!("/api/comments/{encoded_id}/resolve");
    let comment: Value = if resolve {
        client
            .post_json(&path, &Value::Null)
            .await
            .context("resolve comment")?
    } else {
        client
            .delete_json(&path)
            .await
            .context("unresolve comment")?
    };
    let action = if resolve { "resolved" } else { "unresolved" };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&comment)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput {
        stdout,
        stderr: format!("Comment {comment_id} {action}.\n"),
    })
}
