use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use super::{
    display_id, format_metadata_value, format_table, is_canonical_uuid, load_issue_actor_names,
    new_api_client, normalize_assignee_input, resolve_current_workspace_id, resolve_issue_ref,
    retry_actor_get, value_string, ApiClient, Cli, Environment, IssueActorNames,
    IssuePropertyListArgs, IssuePropertyMutationArgs, IssuePropertyUnsetArgs, OutputFormat,
    RunOutput,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PropertyOption {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(default)]
    pub(super) color: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct PropertyConfig {
    #[serde(default)]
    pub(super) options: Vec<PropertyOption>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PropertyDefinition {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(rename = "type")]
    pub(super) property_type: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) icon: String,
    #[serde(default)]
    pub(super) config: PropertyConfig,
    #[serde(default)]
    pub(super) position: f64,
    #[serde(default)]
    pub(super) archived: bool,
    #[serde(default)]
    pub(super) usage_count: i64,
    #[serde(default)]
    pub(super) created_at: String,
    #[serde(default)]
    pub(super) updated_at: String,
}

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

async fn resolve_property_member(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<String> {
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required to resolve assignees; use --workspace-id or set PATCHBAY_WORKSPACE_ID"
        );
    }
    let token = raw.trim();
    if let Some(id) = token.strip_prefix("member:") {
        let id = id.trim();
        if !is_canonical_uuid(id) {
            bail!("actor id in {token:?} must be a UUID");
        }
        return Ok(format!("member:{id}"));
    }
    let input = normalize_assignee_input(token);
    if input.is_empty() {
        bail!("actor value cannot be empty");
    }
    let members =
        retry_actor_get::<Vec<Value>>(client, &format!("/api/workspaces/{workspace_id}/members"))
            .await
            .context("fetch members")?;
    let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
    for member in &members {
        let id = value_string(member, "user_id");
        let name = value_string(member, "name");
        let email = value_string(member, "email");
        if id.eq_ignore_ascii_case(&input)
            || display_id(&id, false).eq_ignore_ascii_case(&input)
            || (!email.is_empty() && email.eq_ignore_ascii_case(&input))
        {
            buckets[0].push((id, name));
        } else if name.eq_ignore_ascii_case(&input) {
            buckets[1].push((id, name));
        } else if name
            .to_ascii_lowercase()
            .contains(&input.to_ascii_lowercase())
        {
            buckets[2].push((id, name));
        }
    }
    for bucket in buckets {
        match bucket.as_slice() {
            [] => {}
            [(id, _)] => return Ok(format!("member:{id}")),
            matches => {
                let matches = matches
                    .iter()
                    .map(|(id, name)| format!("  member {name:?} ({})", display_id(id, false)))
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!("ambiguous assignee {input:?}; matches:\n{matches}");
            }
        }
    }
    bail!("no member found matching {input:?}")
}

async fn encode_issue_property_value(
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

fn actor_property_inputs(
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

fn format_issue_property_value(
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
    .unwrap_or_else(|| format_metadata_value(Some(value)))
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
    let properties = fetch_property_definitions(&client).await?;
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
    let properties = fetch_property_definitions(&client).await?;
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
    let properties = fetch_property_definitions(&client).await?;
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
#[derive(Debug, Args)]
pub(super) struct PropertyArgs {
    #[command(subcommand)]
    pub(super) command: PropertyCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum PropertyCommand {
    #[command(about = "List property definitions")]
    List {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        #[arg(long, help = "Include archived properties")]
        include_archived: bool,
    },
    #[command(about = "Show one property definition")]
    Get {
        #[arg(value_name = "ID-OR-NAME")]
        property: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
    #[command(about = "Create a property definition (workspace owner/admin only)")]
    Create(PropertyCreateArgs),
    #[command(about = "Update a property definition (owner/admin only; type is immutable)")]
    Update(PropertyUpdateArgs),
    #[command(about = "Archive a property definition (hidden from pickers; values preserved)")]
    Archive(PropertyArchiveArgs),
    #[command(about = "Restore an archived property definition")]
    Unarchive(PropertyArchiveArgs),
}

#[derive(Debug, Args)]
pub(super) struct PropertyCreateArgs {
    #[arg(long, help = "Property name (required)")]
    pub(super) name: Option<String>,
    #[arg(
        long = "type",
        help = "Property type: text, number, select, multi_select, date, checkbox, url, actor, multi_actor (required)"
    )]
    pub(super) property_type: Option<String>,
    #[arg(long, default_value = "", help = "Property description")]
    pub(super) description: String,
    #[arg(
        long,
        default_value = "",
        help = "Property icon key from the Web picker (for example, flag, tag, or shield)"
    )]
    pub(super) icon: String,
    #[arg(long, action = clap::ArgAction::Append, help = "Select option as \"Name\" or \"Name:#rrggbb\" (repeatable; select types only)")]
    pub(super) option: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct PropertyUpdateArgs {
    #[arg(value_name = "ID-OR-NAME")]
    pub(super) property: String,
    #[arg(long, help = "New property name")]
    pub(super) name: Option<String>,
    #[arg(long, help = "New property description")]
    pub(super) description: Option<String>,
    #[arg(
        long,
        help = "New property icon key from the Web picker; pass an empty value to clear"
    )]
    pub(super) icon: Option<String>,
    #[arg(long, action = clap::ArgAction::Append, help = "Replacement option list as \"Name\" or \"Name:#rrggbb\" (repeatable)")]
    pub(super) option: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct PropertyArchiveArgs {
    #[arg(value_name = "ID-OR-NAME")]
    pub(super) property: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}
