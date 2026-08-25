use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;

use super::property_models::PropertyConfig;
pub(super) use super::property_models::{PropertyDefinition, PropertyOption};
pub(super) use super::property_read_commands::{
    fetch_property_definitions, format_property_definitions, list_property_definitions,
    resolve_property, run_property_get, run_property_list,
};
use super::{
    new_api_client, Cli, Environment, OutputFormat, PropertyArchiveArgs, PropertyCreateArgs,
    PropertyUpdateArgs, RunOutput,
};

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
            let mut option = serde_json::Map::from_iter([
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

fn format_property_mutation(
    property: &PropertyDefinition,
    output: OutputFormat,
    action: &str,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(property)?)),
        OutputFormat::Table => Ok(format!(
            "Property {:?} {action}.\n{}",
            property.name,
            format_property_definitions(std::slice::from_ref(property), OutputFormat::Table)?
        )),
    }
}

pub(super) async fn run_property_create(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyCreateArgs,
) -> Result<RunOutput> {
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
    let mut body = serde_json::Map::from_iter([
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
    let client = new_api_client(cli, environment)?;
    let property: PropertyDefinition = client
        .post_json("/api/properties", &body)
        .await
        .context("create property")?;
    Ok(RunOutput {
        stdout: format_property_mutation(&property, args.output, "created")?,
        stderr: String::new(),
    })
}

pub(super) async fn run_property_update(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyUpdateArgs,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, &args.property)?;
    let mut body = serde_json::Map::new();
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
    let updated: PropertyDefinition = client
        .patch_json(&format!("/api/properties/{}", property.id), &body)
        .await
        .context("update property")?;
    Ok(RunOutput {
        stdout: format_property_mutation(&updated, args.output, "updated")?,
        stderr: String::new(),
    })
}

pub(super) async fn run_property_archive(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyArchiveArgs,
    archive: bool,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let properties = fetch_property_definitions(&client).await?;
    let property = resolve_property(&properties, &args.property)?;
    let action = if archive { "archive" } else { "unarchive" };
    let updated: PropertyDefinition = client
        .patch_json(
            &format!("/api/properties/{}", property.id),
            &serde_json::json!({"archived":archive}),
        )
        .await
        .with_context(|| format!("{action} property"))?;
    Ok(RunOutput {
        stdout: match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&updated)?),
            OutputFormat::Table => format!(
                "Property {:?} {}.\n",
                updated.name,
                if archive { "archived" } else { "restored" }
            ),
        },
        stderr: String::new(),
    })
}
