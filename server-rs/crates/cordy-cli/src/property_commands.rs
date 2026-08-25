use anyhow::{Context, Result};

pub(super) use super::property_models::{PropertyDefinition, PropertyOption};
pub(super) use super::property_mutation_input::{
    build_property_create_body, build_property_update_body, parse_property_options,
};
use super::property_mutation_output::{format_property_archive, format_property_mutation};
pub(super) use super::property_read_commands::{
    fetch_property_definitions, format_property_definitions, list_property_definitions,
    resolve_property, run_property_get, run_property_list,
};
use super::{
    new_api_client, Cli, Environment, OutputFormat, PropertyArchiveArgs, PropertyCreateArgs,
    PropertyUpdateArgs, RunOutput,
};

pub(super) async fn run_property_create(
    cli: &Cli,
    environment: &Environment,
    args: &PropertyCreateArgs,
) -> Result<RunOutput> {
    let body = build_property_create_body(args)?;
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
    let body = build_property_update_body(args, property)?;
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
        stdout: format_property_archive(&updated, args.output, archive)?,
        stderr: String::new(),
    })
}
