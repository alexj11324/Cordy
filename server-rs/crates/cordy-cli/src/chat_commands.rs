use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use url::form_urlencoded;

use super::{
    http_timeout, new_api_client, value_string, Cli, Environment, OutputFormat, RunOutput,
};

fn chat_reply_count(message: &Value) -> String {
    message
        .get("reply_count")
        .and_then(Value::as_f64)
        .filter(|count| *count != 0.0)
        .map(|count| (count as i64).to_string())
        .unwrap_or_default()
}

fn format_chat_read(response: &Value, output: OutputFormat, overview: bool) -> Result<String> {
    if output == OutputFormat::Json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(response)?));
    }
    if let Some(note) = response
        .get("note")
        .and_then(Value::as_str)
        .filter(|note| !note.is_empty())
    {
        return Ok(format!("{note}\n"));
    }
    let messages = response
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rows = vec![if overview {
        vec![
            "TS".into(),
            "ROLE".into(),
            "AUTHOR".into(),
            "THREAD_ID".into(),
            "REPLIES".into(),
            "TEXT".into(),
        ]
    } else {
        vec!["TS".into(), "ROLE".into(), "AUTHOR".into(), "TEXT".into()]
    }];
    rows.extend(messages.iter().map(|message| {
        let mut row = vec![
            value_string(message, "ts"),
            value_string(message, "role"),
            value_string(message, "author"),
        ];
        if overview {
            row.push(value_string(message, "thread_id"));
            row.push(chat_reply_count(message));
        }
        row.push(value_string(message, "text"));
        row
    }));
    Ok(super::format_table(&rows))
}

pub(super) async fn run_chat_read(
    cli: &Cli,
    environment: &Environment,
    base_path: &str,
    thread_id: Option<&str>,
    args: &ChatReadArgs,
    overview: bool,
) -> Result<RunOutput> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if let Some(before) = args.before.as_deref().filter(|before| !before.is_empty()) {
        serializer.append_pair("before", before);
    }
    if let Some(thread_id) = thread_id.filter(|thread_id| !thread_id.is_empty()) {
        serializer.append_pair("id", thread_id);
    }
    if args.limit > 0 {
        serializer.append_pair("limit", &args.limit.to_string());
    }
    let query = serializer.finish();
    let path = if query.is_empty() {
        base_path.into()
    } else {
        format!("{base_path}?{query}")
    };
    let client = new_api_client(cli, environment)?;
    let response: Value = client.get_json(&path).await.context("read chat")?;
    Ok(RunOutput {
        stdout: format_chat_read(&response, args.output, overview)?,
        stderr: String::new(),
    })
}

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

pub(super) async fn run_attachment_download(
    cli: &Cli,
    environment: &Environment,
    attachment_id: &str,
    output_dir: &Path,
) -> Result<RunOutput> {
    let request_timeout =
        http_timeout(environment.raw("CORDY_HTTP_TIMEOUT")).max(std::time::Duration::from_secs(60));
    let client = new_api_client(cli, environment)?.with_request_timeout(request_timeout);
    let attachment: Value = client
        .get_json(&format!("/api/attachments/{attachment_id}"))
        .await
        .context("get attachment")?;
    let download_url = value_string(&attachment, "download_url");
    if download_url.is_empty() {
        bail!("attachment has no download URL");
    }
    let raw_filename = value_string(&attachment, "filename");
    let filename = Path::new(&raw_filename)
        .file_name()
        .and_then(|filename| filename.to_str())
        .filter(|filename| !filename.is_empty() && *filename != ".")
        .unwrap_or(attachment_id);
    let data = client
        .download_file(&download_url)
        .await
        .context("download file")?;
    let directory = if output_dir.is_absolute() {
        output_dir.to_path_buf()
    } else {
        environment.current_dir().join(output_dir)
    };
    if !output_dir.as_os_str().is_empty() {
        fs::create_dir_all(&directory).context("create output directory")?;
    }
    let destination = directory.join(filename);
    fs::write(&destination, data).context("write file")?;
    let absolute = fs::canonicalize(&destination).unwrap_or(destination);
    let path = absolute.to_string_lossy();
    Ok(RunOutput {
        stdout: format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({
                "id":value_string(&attachment, "id"),
                "filename":filename,
                "path":path,
                "size":value_string(&attachment, "size_bytes")
            }))?
        ),
        stderr: format!("Downloaded: {path}\n"),
    })
}
#[derive(Debug, Args)]
pub(super) struct AttachmentArgs {
    #[command(subcommand)]
    pub(super) command: AttachmentCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum AttachmentCommand {
    #[command(about = "Download an attachment to a local file")]
    Download {
        #[arg(value_name = "ATTACHMENT-ID")]
        attachment_id: String,
        #[arg(
            short = 'o',
            long,
            default_value = ".",
            help = "Directory to save the downloaded file"
        )]
        output_dir: PathBuf,
    },
    #[command(about = "Upload a file to attach to your chat reply")]
    Upload {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        #[arg(long, help = "Chat task id to attach to (defaults to CORDY_TASK_ID)")]
        task: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(super) struct ChatArgs {
    #[command(subcommand)]
    pub(super) command: ChatCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ChatCommand {
    #[command(about = "Overview of the channel this conversation is in (messages + thread list)")]
    History(ChatReadArgs),
    #[command(about = "Read one thread's messages (the current thread, or a specific id)")]
    Thread(ChatThreadArgs),
}

#[derive(Debug, Args)]
pub(super) struct ChatReadArgs {
    #[arg(
        long,
        default_value_t = 0,
        help = "Maximum number of messages to return (the server clamps the range)"
    )]
    pub(super) limit: i64,
    #[arg(
        long,
        help = "Opaque cursor (a next_cursor from a prior page) to read older messages"
    )]
    pub(super) before: Option<String>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct ChatThreadArgs {
    #[arg(value_name = "ID")]
    pub(super) id: Option<String>,
    #[command(flatten)]
    pub(super) read: ChatReadArgs,
}
