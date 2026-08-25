use serde::Serialize;
use serde_json::Value;

use super::issue_property_values::format_issue_property_value;
use super::property_models::PropertyDefinition;
use super::{format_table, IssueActorNames, OutputFormat};

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
) -> anyhow::Result<String> {
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
