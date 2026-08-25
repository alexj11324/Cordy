use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::property_models::PropertyDefinition;
use super::{format_table, new_api_client, ApiClient, Cli, Environment, OutputFormat, RunOutput};

pub(super) async fn fetch_property_definitions(
    client: &ApiClient,
) -> Result<Vec<PropertyDefinition>> {
    list_property_definitions(client, true).await
}

pub(super) async fn list_property_definitions(
    client: &ApiClient,
    include_archived: bool,
) -> Result<Vec<PropertyDefinition>> {
    let path = if include_archived {
        "/api/properties?include_archived=true"
    } else {
        "/api/properties"
    };
    let result: Value = client.get_json(path).await.context("list properties")?;
    serde_json::from_value(
        result
            .get("properties")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .context("decode properties")
}

pub(super) fn format_property_definitions(
    properties: &[PropertyDefinition],
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(properties)?)),
        OutputFormat::Table => {
            let mut rows = vec![vec![
                "ID".into(),
                "ICON".into(),
                "NAME".into(),
                "TYPE".into(),
                "OPTIONS".into(),
                "USED".into(),
                "ARCHIVED".into(),
            ]];
            rows.extend(properties.iter().map(|property| {
                vec![
                    property.id.clone(),
                    property.icon.clone(),
                    property.name.clone(),
                    property.property_type.clone(),
                    property
                        .config
                        .options
                        .iter()
                        .map(|option| option.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    property.usage_count.to_string(),
                    if property.archived {
                        "yes".into()
                    } else {
                        String::new()
                    },
                ]
            }));
            Ok(format_table(&rows))
        }
    }
}

pub(super) async fn run_property_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
    include_archived: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = list_property_definitions(&client, include_archived).await?;
    Ok(RunOutput {
        stdout: format_property_definitions(&properties, output)?,
        stderr: String::new(),
    })
}

pub(super) async fn run_property_get(
    cli: &Cli,
    environment: &Environment,
    property: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, property)?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(property)?),
            OutputFormat::Table => {
                format_property_definitions(std::slice::from_ref(property), output)?
            }
        },
        stderr: String::new(),
    })
}

pub(super) fn resolve_property<'a>(
    properties: &'a [PropertyDefinition],
    reference: &str,
) -> Result<&'a PropertyDefinition> {
    if let Some(property) = properties.iter().find(|property| property.id == reference) {
        return Ok(property);
    }
    let reference = reference.trim();
    if let Some(property) = properties
        .iter()
        .find(|property| property.name.eq_ignore_ascii_case(reference))
    {
        return Ok(property);
    }
    bail!(
        "property {reference:?} not found; available: {}",
        properties
            .iter()
            .map(|property| property.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}
