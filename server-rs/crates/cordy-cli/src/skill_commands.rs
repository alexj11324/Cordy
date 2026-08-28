use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use url::form_urlencoded;

use super::{
    command_output_error, encoded_path_segment, ensure_file_within_workdir, format_table,
    new_api_client, read_setup_confirmation, value_string, Cli, Environment, HttpError,
    OutputFormat, RunOutput, SkillCreateArgs, SkillDeleteArgs, SkillFilesDeleteArgs,
    SkillFilesListArgs, SkillFilesUpsertArgs, SkillGetArgs, SkillImportArgs, SkillRefreshArgs,
    SkillSearchArgs, SkillUpdateArgs,
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

const MAX_SKILL_ARCHIVE_UPLOAD_SIZE: u64 = 16 << 20;
const SKILL_IMPORT_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) async fn run_skill_import(
    cli: &Cli,
    environment: &Environment,
    args: &SkillImportArgs,
) -> Result<RunOutput> {
    let url = args
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let file = args
        .file
        .as_deref()
        .filter(|path| !path.as_os_str().is_empty());
    match (url, file) {
        (None, None) => bail!("either --url or --file is required"),
        (Some(_), Some(_)) => bail!("--url and --file are mutually exclusive"),
        _ => {}
    }
    if !valid_skill_import_conflict_strategy(&args.on_conflict) {
        bail!("--on-conflict must be one of: fail, overwrite, rename, skip");
    }

    let client = new_api_client(cli, environment)?.with_request_timeout(SKILL_IMPORT_TIMEOUT);
    let result: Result<Value> = if let Some(path) = file {
        let (archive, filename) = read_skill_archive(path, environment, args.allow_external_file)?;
        client
            .import_skill_file(archive, &filename, &args.on_conflict)
            .await
    } else {
        client
            .post_json(
                "/api/skills/import",
                &serde_json::json!({
                    "url": url.expect("validated URL").to_owned(),
                    "on_conflict": args.on_conflict,
                }),
            )
            .await
    };

    match result {
        Ok(result) => format_skill_import_result(&result, args.output),
        Err(error) => {
            if let Some(output) = skill_import_error_output(&error, args.output) {
                return Err(command_output_error(output, error));
            }
            Err(error).context("import skill")
        }
    }
}

pub(super) fn valid_skill_import_conflict_strategy(strategy: &str) -> bool {
    matches!(strategy, "fail" | "overwrite" | "rename" | "skip")
}

pub(super) fn read_skill_archive(
    path: &Path,
    environment: &Environment,
    allow_external_file: bool,
) -> Result<(Vec<u8>, String)> {
    ensure_file_within_workdir(path, environment.current_dir(), allow_external_file, "file")?;
    let read_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        environment.current_dir().join(path)
    };
    let metadata = fs::metadata(&read_path).context("read skill archive metadata")?;
    if metadata.len() > MAX_SKILL_ARCHIVE_UPLOAD_SIZE {
        bail!("skill archive exceeds the 16 MiB upload limit");
    }
    let file = fs::File::open(&read_path).context("read skill archive")?;
    let mut archive =
        Vec::with_capacity(metadata.len().min(MAX_SKILL_ARCHIVE_UPLOAD_SIZE) as usize);
    file.take(MAX_SKILL_ARCHIVE_UPLOAD_SIZE + 1)
        .read_to_end(&mut archive)
        .context("read skill archive")?;
    if archive.len() as u64 > MAX_SKILL_ARCHIVE_UPLOAD_SIZE {
        bail!("skill archive exceeds the 16 MiB upload limit");
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("skill.zip")
        .to_owned();
    Ok((archive, filename))
}

pub(super) fn format_skill_import_result(
    result: &Value,
    output: OutputFormat,
) -> Result<RunOutput> {
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(result)?),
        OutputFormat::Table => format_skill_import_table(result),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn skill_import_error_output(
    error: &anyhow::Error,
    output: OutputFormat,
) -> Option<RunOutput> {
    let http = error.downcast_ref::<HttpError>()?;
    if http.body.trim().is_empty() {
        return None;
    }
    let mut payload: Value = serde_json::from_str(&http.body).ok()?;
    if !payload.get("status").is_some() && payload.get("existing_skill").is_some() {
        let reason = {
            let message = value_string(&payload, "error");
            if message.is_empty() {
                "a skill with this name already exists".to_owned()
            } else {
                message
            }
        };
        let existing_skill = payload
            .get("existing_skill")
            .cloned()
            .unwrap_or(Value::Null);
        payload = serde_json::json!({
            "status": "conflict",
            "reason": format!(
                "{reason}; use --on-conflict overwrite to replace it or --on-conflict rename to import a copy"
            ),
            "existing_skill": existing_skill,
        });
    } else if !payload.get("status").is_some() {
        return None;
    }
    format_skill_import_result(&payload, output).ok()
}

fn nested_skill_import_string(object: &Value, parent: &str, key: &str) -> String {
    object
        .get(parent)
        .and_then(|value| value.get(key))
        .map(|value| match value {
            Value::Null => String::new(),
            Value::String(value) => value.clone(),
            value => value.to_string(),
        })
        .unwrap_or_default()
}

pub(super) fn format_skill_import_table(result: &Value) -> String {
    let status = value_string(result, "status");
    let reason = value_string(result, "reason");
    let line = match status.as_str() {
        "" => format!(
            "Skill imported: {} ({})",
            value_string(result, "name"),
            value_string(result, "id")
        ),
        "created" => format!(
            "Skill imported: {} ({})",
            nested_skill_import_string(result, "skill", "name"),
            nested_skill_import_string(result, "skill", "id")
        ),
        "updated" => format!(
            "Skill updated: {} ({})",
            nested_skill_import_string(result, "skill", "name"),
            nested_skill_import_string(result, "skill", "id")
        ),
        "skipped" => format!(
            "Skill skipped: {} ({})",
            nested_skill_import_string(result, "existing_skill", "name"),
            nested_skill_import_string(result, "existing_skill", "id")
        ),
        "conflict" => format!(
            "Skill import conflict: {} ({})",
            nested_skill_import_string(result, "existing_skill", "name"),
            nested_skill_import_string(result, "existing_skill", "id")
        ),
        "failed" => format!("Skill import failed: {reason}"),
        other => format!("Skill import {other}"),
    };
    if reason.is_empty() || status == "failed" {
        format!("{line}\n")
    } else {
        format!("{line}\nReason: {reason}\n")
    }
}

pub(super) async fn run_skill_refresh(
    cli: &Cli,
    environment: &Environment,
    args: &SkillRefreshArgs,
) -> Result<RunOutput> {
    let skill_id = args.skill_id.trim();
    if skill_id.is_empty() {
        bail!("skill ID must not be empty");
    }
    let client = new_api_client(cli, environment)?;
    let result: Value = client
        .post_json(
            &format!("/api/skills/{}/refresh", encoded_path_segment(skill_id)),
            &serde_json::json!({}),
        )
        .await
        .context("refresh skill")?;
    Ok(match args.output {
        OutputFormat::Json => RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&result)?),
            stderr: String::new(),
        },
        OutputFormat::Table => RunOutput {
            stdout: format!(
                "Skill updated from source: {} ({})\n",
                value_string(&result, "name"),
                value_string(&result, "id")
            ),
            stderr: String::new(),
        },
    })
}

pub(super) async fn run_skill_search(
    cli: &Cli,
    environment: &Environment,
    args: &SkillSearchArgs,
) -> Result<RunOutput> {
    let query = args.query.trim();
    if query.is_empty() {
        bail!("query is required");
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", query);
    let client = new_api_client(cli, environment)?;
    let results: Vec<Value> = client
        .get_json(&format!("/api/skills/search?{}", serializer.finish()))
        .await
        .context("search skills")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&results)?),
        OutputFormat::Table => format_skill_search_table(&results),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) fn format_skill_search_table(results: &[Value]) -> String {
    let mut rows = vec![vec![
        "NAME".into(),
        "URL".into(),
        "SOURCE".into(),
        "INSTALLS".into(),
        "DESCRIPTION".into(),
    ]];
    rows.extend(results.iter().map(|result| {
        vec![
            value_string(result, "name"),
            value_string(result, "url"),
            value_string(result, "source"),
            value_string(result, "install_count"),
            value_string(result, "description"),
        ]
    }));
    format_table(&rows)
}

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
