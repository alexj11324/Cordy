use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;

use super::{
    format_table, http_timeout, new_api_client, value_string, Cli, Environment, OutputFormat,
    RunOutput,
};

pub(super) async fn run_agent_lifecycle(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    action: &str,
    past_tense: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let agent: Value = client
        .post_json(&format!("/api/agents/{id}/{action}"), &Value::Null)
        .await
        .with_context(|| format!("{action} agent"))?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&agent)?),
        OutputFormat::Table => format!(
            "Agent {past_tense}: {} ({})\n",
            value_string(&agent, "name"),
            value_string(&agent, "id")
        ),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_agent_tasks(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    output: OutputFormat,
) -> Result<RunOutput> {
    let client = new_api_client(cli, environment)?;
    let tasks: Vec<Value> = client
        .get_json(&format!("/api/agents/{id}/tasks"))
        .await
        .context("list agent tasks")?;
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&tasks)?),
        OutputFormat::Table => {
            let mut rows = vec![vec![
                "ID".into(),
                "ISSUE_ID".into(),
                "STATUS".into(),
                "CREATED_AT".into(),
            ]];
            rows.extend(tasks.iter().map(|task| {
                vec![
                    value_string(task, "id"),
                    value_string(task, "issue_id"),
                    value_string(task, "status"),
                    value_string(task, "created_at"),
                ]
            }));
            format_table(&rows)
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

pub(super) async fn run_agent_avatar(
    cli: &Cli,
    environment: &Environment,
    id: &str,
    file: Option<&Path>,
    output: OutputFormat,
) -> Result<RunOutput> {
    let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(Duration::from_secs(60));
    let client = new_api_client(cli, environment)?.with_request_timeout(timeout);
    let file = file.context("--file is required")?;
    let file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        environment.current_dir().join(file)
    };
    let metadata = fs::metadata(&file).context("file not found")?;
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();
    if !matches!(
        extension.as_str(),
        ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp"
    ) {
        bail!(
            "unsupported file format {:?}: must be .png, .jpg, .jpeg, .gif, or .webp",
            extension
        );
    }
    const MAX_AVATAR_SIZE: u64 = 5 << 20;
    if metadata.len() > MAX_AVATAR_SIZE {
        bail!("file too large: {} bytes (max 5MB)", metadata.len());
    }
    let file_data = fs::read(&file).context("read file")?;
    if file_data.len() as u64 > MAX_AVATAR_SIZE {
        bail!("file too large: {} bytes (max 5MB)", file_data.len());
    }

    let _: Value = client
        .get_json(&format!("/api/agents/{id}"))
        .await
        .context("get agent")?;
    let filename = file.to_string_lossy();
    let upload = client
        .upload_file_with_url(file_data, &filename)
        .await
        .context("upload avatar")?;
    let attachment_id = upload.id;
    let avatar_url = upload.url;
    let _: Value = client
        .put_json(
            &format!("/api/agents/{id}"),
            &serde_json::json!({"avatar_url":&avatar_url}),
        )
        .await
        .context("update agent avatar")?;
    let result = serde_json::json!({
        "id":&attachment_id,
        "agent_id":id,
        "avatar_url":&avatar_url,
    });
    let stdout = match output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&result)?),
        OutputFormat::Table => format_table(&[
            vec!["ID".into(), "AGENT_ID".into(), "AVATAR_URL".into()],
            vec![attachment_id, id.into(), avatar_url],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}
