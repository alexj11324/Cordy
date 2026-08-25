use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

use super::{Cli, Environment, RunOutput, http_timeout, new_api_client};

fn escape_markdown_label(label: &str) -> String {
    let mut escaped = String::with_capacity(label.len());
    for character in label.chars() {
        if matches!(character, '\\' | '[' | ']' | '(' | ')') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub(super) async fn run_attachment_upload(
    cli: &Cli,
    environment: &Environment,
    path: &Path,
    task: Option<&str>,
) -> Result<RunOutput> {
    let task_id = task
        .filter(|task| !task.is_empty())
        .or_else(|| environment.raw("CORDY_TASK_ID"))
        .unwrap_or_default();
    if task_id.is_empty() {
        bail!(
            "no chat task in context: run inside a chat task (CORDY_TASK_ID set) or pass --task <id>"
        );
    }
    let path_text = path.to_string_lossy();
    if path_text.starts_with("http://") || path_text.starts_with("https://") {
        bail!("upload accepts a local file path, not a URL: {path_text}");
    }
    let read_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        environment.current_dir().join(path)
    };
    let data =
        fs::read(&read_path).with_context(|| format!("read file {}", path.to_string_lossy()))?;
    let request_timeout =
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(std::time::Duration::from_secs(60));
    let client = new_api_client(cli, environment)?.with_request_timeout(request_timeout);
    let attachment = client
        .upload_chat_attachment(data, &path_text, task_id)
        .await
        .context("upload attachment")?;
    let filename = path
        .file_name()
        .and_then(|filename| filename.to_str())
        .unwrap_or(&path_text);
    let label = escape_markdown_label(filename);
    let markdown = if attachment.content_type.starts_with("image/") {
        format!("![{label}]({})", attachment.markdown_url)
    } else {
        format!("!file[{label}]({})", attachment.markdown_url)
    };
    Ok(RunOutput {
        stdout: format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "id":attachment.id,
                "filename":filename,
                "markdown_url":attachment.markdown_url,
                "markdown":markdown
            }))?
        ),
        stderr: format!("Uploaded: {filename}\n"),
    })
}
