use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    encoded_path_segment, format_table, new_api_client, value_string, Cli, Environment,
    OutputFormat, RunOutput,
};

pub(super) async fn run_squad_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let squads: Vec<Value> = client
        .get_json("/api/squads")
        .await
        .context("list squads")?;
    if output == OutputFormat::Json {
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&squads)?),
            stderr: String::new(),
        });
    }
    if squads.is_empty() {
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: "No squads found.\n".into(),
        });
    }
    Ok(RunOutput {
        stdout: format_squad_list_table(&squads),
        stderr: String::new(),
    })
}

pub(super) async fn run_squad_get(
    cli: &Cli,
    environment: &Environment,
    squad_id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let squad_id = squad_id.trim();
    if squad_id.is_empty() {
        bail!("squad ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    let squad: Value = client
        .get_json(&format!("/api/squads/{}", encoded_path_segment(squad_id)))
        .await
        .context("get squad")?;
    Ok(RunOutput {
        stdout: match output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&squad)?),
            OutputFormat::Table => format_squad_details_table(&squad),
        },
        stderr: String::new(),
    })
}

pub(super) fn format_squad_list_table(squads: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "LEADER ID".into(),
        "MEMBERS".into(),
    ]];
    rows.extend(squads.iter().map(|squad| {
        vec![
            value_string(squad, "id"),
            value_string(squad, "name"),
            value_string(squad, "leader_id"),
            squad_member_count_display(squad),
        ]
    }));
    format_table(&rows)
}

pub(super) fn squad_member_count_display(squad: &Value) -> String {
    let Some(count) = squad.get("member_count") else {
        return "-".into();
    };
    if let Some(count) = count.as_u64().filter(|count| *count > 0) {
        return count.to_string();
    }
    if let Some(count) = count.as_i64().filter(|count| *count > 0) {
        return count.to_string();
    }
    "-".into()
}

pub(super) fn format_squad_details_table(squad: &Value) -> String {
    let mut output = format!(
        "ID:           {}\nName:         {}\nDescription:  {}\nLeader ID:    {}\nCreated:      {}\n",
        value_string(squad, "id"),
        value_string(squad, "name"),
        value_string(squad, "description"),
        value_string(squad, "leader_id"),
        value_string(squad, "created_at"),
    );
    let instructions = value_string(squad, "instructions");
    if !instructions.is_empty() {
        output.push_str(&format!("Instructions: {}\n", instructions));
    }
    output
}
