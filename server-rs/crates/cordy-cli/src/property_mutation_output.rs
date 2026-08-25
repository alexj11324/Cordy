use anyhow::Result;

use super::property_models::PropertyDefinition;
use super::property_read_commands::format_property_definitions;
use super::OutputFormat;

pub(super) fn format_property_mutation(
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

pub(super) fn format_property_archive(
    property: &PropertyDefinition,
    output: OutputFormat,
    archive: bool,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(property)?)),
        OutputFormat::Table => Ok(format!(
            "Property {:?} {}.\n",
            property.name,
            if archive { "archived" } else { "restored" }
        )),
    }
}
