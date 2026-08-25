use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{
    encoded_path_segment, format_table, new_api_client, value_string, Cli, Environment,
    OutputFormat, RunOutput, SkillGetArgs,
};

pub(super) async fn run_skill_list(
    cli: &Cli,
    environment: &Environment,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let skills: Vec<Value> = client
        .get_json("/api/skills")
        .await
        .context("list skills")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&skills)?),
        OutputFormat::Table => format_skill_list_table(&skills),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_skill_get(
    cli: &Cli,
    environment: &Environment,
    args: &SkillGetArgs,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    let skill: Value = client
        .get_json(&format!("/api/skills/{}", encoded_path_segment(skill_id)))
        .await
        .context("get skill")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&skill)?),
        OutputFormat::Table => format_skill_details_table(&skill),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_skill_list_table(skills: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "NAME".into(),
        "DESCRIPTION".into(),
        "CREATED_AT".into(),
    ]];
    rows.extend(skills.iter().map(|skill| {
        vec![
            value_string(skill, "id"),
            value_string(skill, "name"),
            value_string(skill, "description"),
            value_string(skill, "created_at"),
        ]
    }));
    format_table(&rows)
}

pub(super) fn format_skill_details_table(skill: &Value) -> String {
    format_table(&[
        vec![
            "ID".into(),
            "NAME".into(),
            "DESCRIPTION".into(),
            "CREATED_AT".into(),
        ],
        vec![
            value_string(skill, "id"),
            value_string(skill, "name"),
            value_string(skill, "description"),
            value_string(skill, "created_at"),
        ],
    ])
}
