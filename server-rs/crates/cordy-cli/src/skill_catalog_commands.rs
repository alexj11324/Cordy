use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use url::form_urlencoded;

use super::{
    command_output_error, encoded_path_segment, ensure_file_within_workdir, format_table,
    new_api_client, value_string, Cli, Environment, HttpError, OutputFormat, RunOutput,
    SkillImportArgs, SkillRefreshArgs, SkillSearchArgs,
};

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
    let mut file = fs::File::open(&read_path).context("read skill archive")?;
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
