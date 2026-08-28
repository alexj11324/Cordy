use anyhow::Result;
use serde_json::Value;

use super::{format_table, value_string, OutputFormat, RunOutput};

pub(super) fn output_runtime_profiles(
    profiles: &[Value],
    output: OutputFormat,
    single: bool,
) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        let value = if single {
            &profiles[0]
        } else {
            return Ok(RunOutput {
                stdout: format!("{}\n", serde_json::to_string_pretty(profiles)?),
                stderr: String::new(),
            });
        };
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(value)?),
            stderr: String::new(),
        });
    }
    let mut profiles = profiles.to_vec();
    profiles.sort_by_key(|profile| value_string(profile, "display_name"));
    let mut rows = vec![vec![
        "ID".into(),
        "DISPLAY_NAME".into(),
        "PROTOCOL_FAMILY".into(),
        "COMMAND_NAME".into(),
        "ENABLED".into(),
    ]];
    rows.extend(profiles.iter().map(|profile| {
        vec![
            value_string(profile, "id"),
            value_string(profile, "display_name"),
            value_string(profile, "protocol_family"),
            value_string(profile, "command_name"),
            value_string(profile, "enabled"),
        ]
    }));
    Ok(RunOutput {
        stdout: format_table(&rows),
        stderr: String::new(),
    })
}

pub(super) fn format_runtime_rows(
    values: &[Value],
    output: OutputFormat,
    headers: &[&str],
    fields: &[&str],
) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(values)?));
    }
    let mut rows = vec![headers.iter().map(|header| (*header).into()).collect()];
    rows.extend(values.iter().map(|value| {
        fields
            .iter()
            .map(|field| value_string(value, field))
            .collect()
    }));
    Ok(format_table(&rows))
}
