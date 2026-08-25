use anyhow::Result;
use serde_json::Value;

use super::{value_string, OutputFormat, RunOutput};

pub(super) fn format_runtime_update_result(
    update: &Value,
    output: OutputFormat,
    waited: bool,
) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(update)?),
        OutputFormat::Table if !waited => format!(
            "Update initiated: {} (status: {})\n",
            value_string(update, "id"),
            value_string(update, "status")
        ),
        OutputFormat::Table if value_string(update, "status") == "completed" => {
            format!("Update completed: {}\n", value_string(update, "output"))
        }
        OutputFormat::Table => format!(
            "Update {}: {}\n",
            value_string(update, "status"),
            value_string(update, "error")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
