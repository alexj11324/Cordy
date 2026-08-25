use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::collections::HashMap;

use super::property_models::{PropertyDefinition, PropertyOption};
use super::{PropertyCreateArgs, PropertyUpdateArgs};

const DEFAULT_PROPERTY_OPTION_COLOR: &str = "#6b7280";

pub(super) fn parse_property_options(flags: &[String], existing: &[PropertyOption]) -> Vec<Value> {
    let by_name = existing
        .iter()
        .map(|option| (option.name.to_ascii_lowercase(), option.id.as_str()))
        .collect::<HashMap<_, _>>();
    flags
        .iter()
        .map(|raw| {
            let (name, color) = raw.rfind(":#").filter(|index| *index > 0).map_or_else(
                || (raw.as_str(), DEFAULT_PROPERTY_OPTION_COLOR),
                |index| (&raw[..index], &raw[index + 1..]),
            );
            let name = name.trim();
            let mut option = Map::from_iter([
                ("name".into(), Value::String(name.into())),
                ("color".into(), Value::String(color.into())),
            ]);
            if let Some(id) = by_name.get(&name.to_ascii_lowercase()) {
                option.insert("id".into(), Value::String((*id).into()));
            }
            Value::Object(option)
        })
        .collect()
}

pub(super) fn build_property_create_body(args: &PropertyCreateArgs) -> Result<Map<String, Value>> {
    let name = args
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .context("--name is required")?;
    let property_type = args
        .property_type
        .as_deref()
        .filter(|property_type| !property_type.is_empty())
        .context("--type is required")?;
    let mut body = Map::from_iter([
        ("name".into(), Value::String(name.into())),
        ("type".into(), Value::String(property_type.into())),
        (
            "description".into(),
            Value::String(args.description.clone()),
        ),
        ("icon".into(), Value::String(args.icon.clone())),
    ]);
    if !args.option.is_empty() {
        body.insert(
            "config".into(),
            serde_json::json!({"options":parse_property_options(&args.option, &[])}),
        );
    }
    Ok(body)
}

pub(super) fn build_property_update_body(
    args: &PropertyUpdateArgs,
    property: &PropertyDefinition,
) -> Result<Map<String, Value>> {
    let mut body = Map::new();
    for (key, value) in [
        ("name", args.name.as_ref()),
        ("description", args.description.as_ref()),
        ("icon", args.icon.as_ref()),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if !args.option.is_empty() {
        body.insert(
            "config".into(),
            serde_json::json!({
                "options":parse_property_options(&args.option, &property.config.options)
            }),
        );
    }
    if body.is_empty() {
        bail!("nothing to update; pass --name, --description, --icon, or --option");
    }
    Ok(body)
}
