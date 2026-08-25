//! Read-only issue metadata commands.
//!
//! Metadata mutations and value parsing remain separate; this module owns the
//! list/get HTTP reads and their 404/field presentation semantics.

use anyhow::{Context, Result};
use serde_json::Value;

use super::issue_metadata_output::{format_metadata_output, metadata_object, metadata_value_type};
use super::{
    format_metadata_value, format_table, new_api_client, resolve_issue_ref, Cli, Environment,
    HttpError, IssueMetadataKeyArgs, IssueMetadataListArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_metadata_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result = client
        .get_json::<Value>(&format!("/api/issues/{issue_id}/metadata"))
        .await;
    let metadata = match result {
        Ok(result) => metadata_object(&result),
        Err(error)
            if error
                .downcast_ref::<HttpError>()
                .is_some_and(|error| error.status_code == 404) =>
        {
            serde_json::Map::new()
        }
        Err(error) => return Err(error).context("list metadata"),
    };
    Ok(RunOutput {
        stdout: format_metadata_output(&metadata, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_metadata_get(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataKeyArgs,
) -> Result<RunOutput> {
    let key = args
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .context("--key is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .get_json(&format!("/api/issues/{issue_id}/metadata"))
        .await
        .context("get metadata")?;
    let metadata = metadata_object(&result);
    let value = metadata
        .get(key)
        .with_context(|| format!("key {key:?} not found on issue"))?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(value)?),
        OutputFormat::Table => format_table(&[
            vec!["KEY".into(), "VALUE".into(), "TYPE".into()],
            vec![
                key.into(),
                format_metadata_value(Some(value)),
                metadata_value_type(value).into(),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
