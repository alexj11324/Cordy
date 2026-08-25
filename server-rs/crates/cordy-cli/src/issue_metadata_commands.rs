use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    format_metadata_value, format_table, new_api_client, resolve_issue_ref, Cli, Environment,
    HttpError, IssueMetadataDeleteArgs, IssueMetadataKeyArgs, IssueMetadataListArgs,
    IssueMetadataSetArgs, OutputFormat, RunOutput,
};

fn metadata_object(result: &Value) -> serde_json::Map<String, Value> {
    result
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn metadata_value_type(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        _ => "unknown",
    }
}

pub(super) fn format_metadata_table(metadata: &serde_json::Map<String, Value>) -> String {
    let mut keys = metadata.keys().collect::<Vec<_>>();
    keys.sort();
    let mut rows = vec![vec!["KEY".into(), "VALUE".into(), "TYPE".into()]];
    rows.extend(keys.into_iter().map(|key| {
        let value = &metadata[key];
        vec![
            key.clone(),
            format_metadata_value(Some(value)),
            metadata_value_type(value).into(),
        ]
    }));
    format_table(&rows)
}

fn format_metadata_output(
    metadata: &serde_json::Map<String, Value>,
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(metadata)?)),
        OutputFormat::Table => Ok(format_metadata_table(metadata)),
    }
}

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
