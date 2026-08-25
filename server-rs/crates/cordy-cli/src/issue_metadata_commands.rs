use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::issue_metadata_output::{format_metadata_output, metadata_object};
use super::{
    new_api_client, resolve_issue_ref, Cli, Environment, IssueMetadataDeleteArgs,
    IssueMetadataSetArgs, OutputFormat, RunOutput,
};

pub(super) fn parse_metadata_value(raw: &str, forced_type: Option<&str>) -> Result<Value> {
    match forced_type.unwrap_or_default() {
        "string" => Ok(Value::String(raw.into())),
        "number" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Number(_)) => Ok(value),
            _ => bail!("value {raw:?} is not a valid number"),
        },
        "bool" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => bail!("value {raw:?} is not a valid bool (expected true or false)"),
        },
        "" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => Ok(value),
            _ => Ok(Value::String(raw.into())),
        },
        value_type => {
            bail!("unknown --type {value_type:?} (expected string, number, or bool)")
        }
    }
}

pub(super) async fn run_issue_metadata_set(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataSetArgs,
) -> Result<RunOutput> {
    let key = args
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .context("--key is required")?;
    let raw = args.value.as_deref().context("--value is required")?;
    let value = parse_metadata_value(raw, args.value_type.as_deref())?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let result: Value = client
        .put_json(
            &format!("/api/issues/{issue_id}/metadata/{key}"),
            &serde_json::json!({"value":value}),
        )
        .await
        .context("set metadata")?;
    let metadata = metadata_object(&result);
    Ok(RunOutput {
        stdout: format_metadata_output(&metadata, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_metadata_delete(
    cli: &Cli,
    environment: &Environment,
    args: &IssueMetadataDeleteArgs,
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
    client
        .delete(&format!("/api/issues/{issue_id}/metadata/{key}"))
        .await
        .context("delete metadata")?;
    let result = client
        .get_json::<Value>(&format!("/api/issues/{issue_id}/metadata"))
        .await;
    let stdout = match result {
        Ok(result) => format_metadata_output(&metadata_object(&result), args.output)?,
        Err(_) if args.output == OutputFormat::Json => "{\n  \"deleted\": true\n}\n".into(),
        Err(_) => "Key deleted.\n".into(),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
