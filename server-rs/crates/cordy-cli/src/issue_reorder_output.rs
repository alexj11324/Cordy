//! Output formatting for issue reorder operations.
//!
//! Reorder validation, column discovery, and position math stay in the command
//! module; this file owns only the stable JSON/table response contract.

use anyhow::Result;
use serde_json::Value;

use super::{format_table, value_string, OutputFormat, RunOutput};

pub(super) fn issue_value_key(issue: &Value) -> String {
    match value_string(issue, "identifier") {
        value if value.is_empty() => value_string(issue, "id"),
        value => value,
    }
}

pub(super) fn issue_reorder_output(
    issue: &Value,
    output: OutputFormat,
    stderr: String,
) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(issue)?),
        OutputFormat::Table => format_table(&[
            vec![
                "KEY".into(),
                "TITLE".into(),
                "STATUS".into(),
                "PRIORITY".into(),
            ],
            vec![
                issue_value_key(issue),
                value_string(issue, "title"),
                value_string(issue, "status"),
                value_string(issue, "priority"),
            ],
        ]),
    };
    Ok(RunOutput { stdout, stderr })
}
