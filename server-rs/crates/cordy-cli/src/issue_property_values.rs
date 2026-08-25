use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::issue_property_actor::resolve_property_member;
use super::property_commands::PropertyDefinition;
use super::{ApiClient, IssueActorNames};

pub(super) async fn encode_issue_property_value(
    client: &ApiClient,
    workspace_id: &str,
    property: &PropertyDefinition,
    raw: &str,
) -> Result<Value> {
    let valid_options = property
        .config
        .options
        .iter()
        .map(|option| option.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let resolve_option = |reference: &str| -> Result<String> {
        let reference = reference.trim();
        property
            .config
            .options
            .iter()
            .find(|option| option.id == reference || option.name.eq_ignore_ascii_case(reference))
            .map(|option| option.id.clone())
            .with_context(|| {
                format!(
                    "option {reference:?} not found on property {:?}; valid options: {valid_options}",
                    property.name
                )
            })
    };
    match property.property_type.as_str() {
        "select" => Ok(Value::String(resolve_option(raw)?)),
        "multi_select" => {
            let values = raw
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(resolve_option)
                .collect::<Result<Vec<_>>>()?;
            if values.is_empty() {
                bail!("--value must list at least one option; valid options: {valid_options}");
            }
            Ok(Value::Array(
                values.into_iter().map(Value::String).collect(),
            ))
        }
        "actor" => Ok(Value::String(
            resolve_property_member(client, workspace_id, raw).await?,
        )),
        "multi_actor" => {
            let mut values = Vec::new();
            for token in raw
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
            {
                values.push(Value::String(
                    resolve_property_member(client, workspace_id, token).await?,
                ));
            }
            if values.is_empty() {
                bail!("--value must list at least one member");
            }
            Ok(Value::Array(values))
        }
        "number" => match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Number(_)) => Ok(value),
            _ => bail!("value {raw:?} is not a valid number"),
        },
        "checkbox" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => bail!("value {raw:?} is not a valid bool (expected true or false)"),
        },
        _ => Ok(Value::String(raw.into())),
    }
}

pub(super) fn actor_property_inputs(
    properties: &[PropertyDefinition],
    bag: &serde_json::Map<String, Value>,
) -> Vec<Value> {
    let mut inputs = Vec::new();
    for property in properties {
        if !matches!(property.property_type.as_str(), "actor" | "multi_actor") {
            continue;
        }
        let Some(value) = bag.get(&property.id) else {
            continue;
        };
        let values = value
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(value));
        for value in values {
            let Some(reference) = value.as_str() else {
                continue;
            };
            let Some((actor_type, actor_id)) = reference.split_once(':') else {
                continue;
            };
            inputs.push(serde_json::json!({"assignee_type":actor_type,"assignee_id":actor_id}));
        }
    }
    inputs
}

pub(super) fn format_issue_property_value(
    property: &PropertyDefinition,
    value: &Value,
    actors: &IssueActorNames,
) -> String {
    let option_name = |id: &str| {
        property
            .config
            .options
            .iter()
            .find(|option| option.id == id)
            .map_or_else(|| id.into(), |option| option.name.clone())
    };
    let actor_name = |reference: &str| {
        actors
            .0
            .get(reference)
            .cloned()
            .unwrap_or_else(|| reference.into())
    };
    match property.property_type.as_str() {
        "select" => value.as_str().map(option_name),
        "multi_select" => value.as_array().map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(option_name)
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "actor" => value.as_str().map(actor_name),
        "multi_actor" => value.as_array().map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(actor_name)
                .collect::<Vec<_>>()
                .join(", ")
        }),
        "checkbox" => value
            .as_bool()
            .map(|checked| if checked { "✓".into() } else { "✗".into() }),
        _ => None,
    }
    .unwrap_or_else(|| super::format_metadata_value(Some(value)))
}
