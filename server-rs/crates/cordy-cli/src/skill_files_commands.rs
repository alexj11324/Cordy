use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;

use super::{
    encoded_path_segment, format_table, new_api_client, resolve_skill_content_sources,
    value_string, Cli, Environment, OutputFormat, RunOutput, SkillFilesDeleteArgs,
    SkillFilesListArgs, SkillFilesUpsertArgs,
};

pub(super) async fn run_skill_files_list(
    cli: &Cli,
    environment: &Environment,
    args: &SkillFilesListArgs,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    let files: Vec<Value> = client
        .get_json(&format!(
            "/api/skills/{}/files",
            encoded_path_segment(skill_id)
        ))
        .await
        .context("list skill files")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&files)?),
        OutputFormat::Table => format_skill_files_table(&files),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_skill_files_upsert<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SkillFilesUpsertArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let path = args.path.as_deref().unwrap_or_default();
    if path.is_empty() {
        bail!("--path is required");
    }
    let content = resolve_skill_content_sources(
        args.content.as_deref(),
        args.content_stdin,
        args.content_file.as_deref(),
        environment,
        input,
    )?;
    let content = content.filter(|content| !content.is_empty());
    let content = content.context("--content is required")?;
    let client = new_api_client(cli, environment)?;
    let result: Value = client
        .put_json(
            &format!("/api/skills/{}/files", encoded_path_segment(skill_id)),
            &serde_json::json!({
                "path": path,
                "content": content,
            }),
        )
        .await
        .context("upsert skill file")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: format!(
                "Skill file upserted: {} ({})\n",
                value_string(&result, "path"),
                value_string(&result, "id")
            ),
            stderr: String::new(),
        },
    })
}

pub(super) async fn run_skill_files_delete(
    cli: &Cli,
    environment: &Environment,
    args: &SkillFilesDeleteArgs,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let file_id = args.file_id.trim();
    if file_id.is_empty() {
        bail!("file ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!(
            "/api/skills/{}/files/{}",
            encoded_path_segment(skill_id),
            encoded_path_segment(file_id)
        ))
        .await
        .context("delete skill file")?;
    Ok(RunOutput {
        stdout: format!("Skill file deleted: {file_id}\n"),
        stderr: String::new(),
    })
}

pub(super) fn format_skill_files_table(files: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "PATH".into(),
        "CREATED_AT".into(),
        "UPDATED_AT".into(),
    ]];
    rows.extend(files.iter().map(|file| {
        vec![
            value_string(file, "id"),
            value_string(file, "path"),
            value_string(file, "created_at"),
            value_string(file, "updated_at"),
        ]
    }));
    format_table(&rows)
}
