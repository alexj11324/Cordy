//! Output and response-shape helpers for issue metadata commands.
//!
//! Metadata parsing and HTTP mutations stay in the command module; this file
//! owns the stable object extraction and JSON/table presentation contract.

use anyhow::Result;
use serde_json::Value;

use super::{format_metadata_value, format_table, OutputFormat};

pub(super) fn metadata_object(result: &Value) -> serde_json::Map<String, Value> {
    result
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn metadata_value_type(value: &Value) -> &'static str {
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

pub(super) fn format_metadata_output(
    metadata: &serde_json::Map<String, Value>,
    output: OutputFormat,
) -> Result<String> {
    match output {
        OutputFormat::Json => Ok(format!("{}\n", serde_json::to_string_pretty(metadata)?)),
        OutputFormat::Table => Ok(format_metadata_table(metadata)),
    }
}
