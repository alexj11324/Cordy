use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use super::issue_property_values::{
    actor_property_inputs, encode_issue_property_value, format_issue_property_value,
};
use super::property_commands::{resolve_property, PropertyDefinition};
use super::{
    format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, ApiClient, Cli, Environment, IssueActorNames, IssuePropertyListArgs,
    IssuePropertyMutationArgs, IssuePropertyUnsetArgs, OutputFormat, RunOutput,
};

#[derive(Debug, Serialize)]
pub(super) struct IssuePropertyRow {
    pub(super) property_id: String,
    pub(super) name: String,
    #[serde(rename = "type")]
    pub(super) property_type: String,
    pub(super) value: Value,
    pub(super) display: String,
    #[serde(skip_serializing_if = "is_false")]
    pub(super) archived: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub(super) fn build_issue_property_rows(
    properties: &[PropertyDefinition],
    bag: &serde_json::Map<String, Value>,
    actors: &IssueActorNames,
) -> Vec<IssuePropertyRow> {
    properties
        .iter()
        .filter_map(|property| {
            let value = bag.get(&property.id)?;
            Some(IssuePropertyRow {
                property_id: property.id.clone(),
                name: property.name.clone(),
                property_type: property.property_type.clone(),
                value: value.clone(),
                display: format_issue_property_value(property, value, actors),
                archived: property.archived,
            })
        })
        .collect()
}

pub(super) fn format_issue_property_rows(
    rows: &[IssuePropertyRow],
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(rows)?)),
        OutputFormat::Table => {
            let mut table = vec![vec!["NAME".into(), "VALUE".into(), "TYPE".into()]];
            table.extend(rows.iter().map(|row| {
                vec![
                    row.name.clone(),
                    row.display.clone(),
                    row.property_type.clone(),
                ]
            }));
            Ok(format_table(&table))
        }
    }
}

async fn property_rows(
    client: &ApiClient,
    workspace_id: &str,
    properties: &[PropertyDefinition],
    bag: &serde_json::Map<String, Value>,
) -> Vec<IssuePropertyRow> {
    let inputs = actor_property_inputs(properties, bag);
    let actors = load_issue_actor_names(client, workspace_id, &inputs).await;
    build_issue_property_rows(properties, bag, &actors)
}

pub(super) async fn run_issue_property_list(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePropertyListArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let properties = super::property_commands::fetch_property_definitions(&client).await?;
    let issue: Value = client
        .get_json(&format!("/api/issues/{issue_id}"))
        .await
        .context("get issue")?;
    let bag = issue
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let rows = property_rows(&client, &workspace_id, &properties, &bag).await;
    Ok(RunOutput {
        stdout: format_issue_property_rows(&rows, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_property_set(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePropertyMutationArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let raw = args.value.as_deref().context("--value is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let properties = super::property_commands::fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, name)?;
    let workspace_id = resolve_current_workspace_id(cli, environment);
    let value = encode_issue_property_value(&client, &workspace_id, property, raw).await?;
    let result: Value = client
        .put_json(
            &format!("/api/issues/{issue_id}/properties/{}", property.id),
            &serde_json::json!({"value":value}),
        )
        .await
        .context("set property")?;
    let bag = result
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let rows = property_rows(&client, &workspace_id, &properties, &bag).await;
    Ok(RunOutput {
        stdout: format_issue_property_rows(&rows, args.output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_issue_property_unset(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePropertyUnsetArgs,
) -> Result<RunOutput> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let properties = super::property_commands::fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, name)?;
    client
        .delete(&format!(
            "/api/issues/{issue_id}/properties/{}",
            property.id
        ))
        .await
        .context("unset property")?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => "{\n  \"deleted\": true\n}\n".into(),
            OutputFormat::Table => format!("Property {:?} unset.\n", property.name),
        },
        stderr: String::new(),
    })
}
