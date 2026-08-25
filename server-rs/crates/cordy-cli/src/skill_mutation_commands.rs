use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;

use super::{
    encoded_path_segment, ensure_file_within_workdir, new_api_client, read_setup_confirmation,
    value_string, Cli, Environment, OutputFormat, RunOutput, SkillCreateArgs, SkillDeleteArgs,
    SkillUpdateArgs,
};

pub(super) fn resolve_skill_content<R: Read>(
    args: &SkillCreateArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    resolve_skill_content_sources(
        args.content.as_deref(),
        args.content_stdin,
        args.content_file.as_deref(),
        environment,
        input,
    )
}

pub(super) fn resolve_skill_content_sources<R: Read>(
    content: Option<&str>,
    content_stdin: bool,
    content_file: Option<&Path>,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let sources = [content.is_some(), content_stdin, content_file.is_some()]
        .into_iter()
        .filter(|source| *source)
        .count();
    if sources > 1 {
        bail!("--content, --content-stdin, and --content-file are mutually exclusive");
    }
    if content_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --content-stdin")?;
        let content = String::from_utf8(bytes).map_err(|_| {
            anyhow::anyhow!("stdin content for --content-stdin must be valid UTF-8")
        })?;
        return Ok(Some(content));
    }
    if let Some(path) = content_file {
        ensure_file_within_workdir(path, environment.current_dir(), false, "content")?;
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(read_path).context("read file for --content-file")?;
        let content = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("file content for --content-file must be valid UTF-8"))?;
        return Ok(Some(content));
    }
    Ok(content.map(str::to_owned))
}

pub(super) async fn run_skill_create<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SkillCreateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let name = args.name.as_deref().filter(|name| !name.is_empty());
    let name = name.context("--name is required")?;
    let content = resolve_skill_content(args, environment, input)?;
    let mut body = serde_json::Map::new();
    body.insert("name".into(), Value::String(name.into()));
    if !args.description.is_empty() {
        body.insert(
            "description".into(),
            Value::String(args.description.clone()),
        );
    }
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        body.insert("content".into(), Value::String(content));
    }
    if let Some(raw_config) = args.config.as_deref() {
        let config: Value = serde_json::from_str(raw_config)
            .map_err(|error| anyhow::anyhow!("--config must be valid JSON: {error}"))?;
        body.insert("config".into(), config);
    }
    let client = new_api_client(cli, environment)?;
    let result: Value = client
        .post_json("/api/skills", &body)
        .await
        .context("create skill")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: format!(
                "Skill created: {} ({})\n",
                value_string(&result, "name"),
                value_string(&result, "id")
            ),
            stderr: String::new(),
        },
    })
}

pub(super) async fn run_skill_update<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SkillUpdateArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let mut body = serde_json::Map::new();
    if let Some(name) = &args.name {
        body.insert("name".into(), Value::String(name.clone()));
    }
    if let Some(description) = &args.description {
        body.insert("description".into(), Value::String(description.clone()));
    }
    if let Some(content) = resolve_skill_content_sources(
        args.content.as_deref(),
        args.content_stdin,
        args.content_file.as_deref(),
        environment,
        input,
    )? {
        body.insert("content".into(), Value::String(content));
    }
    if let Some(raw_config) = args.config.as_deref() {
        let config: Value = serde_json::from_str(raw_config)
            .map_err(|error| anyhow::anyhow!("--config must be valid JSON: {error}"))?;
        body.insert("config".into(), config);
    }
    if body.is_empty() {
        bail!("no fields to update; use --name, --description, --content, or --config");
    }
    let client = new_api_client(cli, environment)?;
    let result: Value = client
        .put_json(
            &format!("/api/skills/{}", encoded_path_segment(skill_id)),
            &body,
        )
        .await
        .context("update skill")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: format!(
                "Skill updated: {} ({})\n",
                value_string(&result, "name"),
                value_string(&result, "id")
            ),
            stderr: String::new(),
        },
    })
}

pub(super) async fn run_skill_delete<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &SkillDeleteArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let prompt =
        format!("Are you sure you want to delete skill {skill_id}? This cannot be undone. [y/N] ");
    let mut stdout = String::new();
    if !args.yes {
        stdout.push_str(&prompt);
        let answer = read_setup_confirmation(input)?;
        if !matches!(answer.as_str(), "y" | "yes") {
            stdout.push_str("Aborted.\n");
            return Ok(RunOutput {
                stdout,
                stderr: String::new(),
            });
        }
    }
    let client = new_api_client(cli, environment)?;
    client
        .delete(&format!("/api/skills/{}", encoded_path_segment(skill_id)))
        .await
        .context("delete skill")?;
    stdout.push_str(&format!("Skill deleted: {skill_id}\n"));
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
