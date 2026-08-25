use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt::Write;
use std::fs;
use std::io::Read;
use std::path::Path;

use super::issue_safety::guard_issue_description_local_links;
use super::{
    collect_local_attachments, ensure_file_within_workdir, http_timeout, new_api_client,
    resolve_issue_ref, trim_one_trailing_newline, unescape_backslash_escapes, Cli, Environment,
    IssueCommentAddArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_comment_add<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssueCommentAddArgs,
    input: &mut R,
) -> Result<RunOutput> {
    let Some(content) = resolve_issue_comment_content(args, environment, input)? else {
        bail!("--content, --content-stdin, or --content-file is required");
    };
    guard_issue_description_local_links(
        &content,
        environment,
        "Deliver the file itself with `cordy issue comment add <issue-id> --attachment <path>` (repeatable) and drop the link.",
    )?;

    let mut client = new_api_client(cli, environment)?;
    if !args.attachment.is_empty() {
        let timeout = http_timeout(environment.raw("CORDY_HTTP_TIMEOUT"))
            .max(std::time::Duration::from_secs(60));
        client = client.with_request_timeout(timeout);
    }
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let (pending, mut stderr) =
        collect_local_attachments(&args.attachment, args.allow_external_file, environment)?;
    let mut attachment_ids = Vec::with_capacity(pending.len());
    for attachment in pending {
        let id = client
            .upload_file(attachment.data, &attachment.path, &issue_id)
            .await
            .with_context(|| format!("upload attachment {}", attachment.path))?;
        attachment_ids.push(id);
        let _ = writeln!(stderr, "Uploaded {}", attachment.path);
    }

    let mut body = serde_json::Map::from_iter([("content".into(), Value::String(content))]);
    if let Some(parent_id) = args.parent.as_deref().filter(|value| !value.is_empty()) {
        body.insert("parent_id".into(), Value::String(parent_id.into()));
    }
    if !attachment_ids.is_empty() {
        body.insert(
            "attachment_ids".into(),
            Value::Array(attachment_ids.into_iter().map(Value::String).collect()),
        );
    }
    let comment: Value = client
        .post_json(&format!("/api/issues/{issue_id}/comments"), &body)
        .await
        .context("add comment")?;
    let _ = writeln!(stderr, "Comment added to issue {}.", args.issue_id);
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&comment)?),
        OutputFormat::Table => String::new(),
    };
    Ok(RunOutput { stdout, stderr })
}

pub(super) fn resolve_issue_comment_content<R: Read>(
    args: &IssueCommentAddArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let inline = args.content.as_deref().unwrap_or_default();
    let content_file = args
        .content_file
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(Path::new);
    let sources = [
        args.content_stdin,
        !inline.is_empty(),
        content_file.is_some(),
    ]
    .into_iter()
    .filter(|source| *source)
    .count();
    if sources > 1 {
        bail!("--content, --content-stdin, and --content-file are mutually exclusive");
    }
    if args.content_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --content-stdin")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --content-stdin is empty");
        }
        return Ok(Some(body));
    }
    if let Some(path) = content_file {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "content",
        )?;
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(read_path).context("read file for --content-file")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --content-file is empty");
        }
        return Ok(Some(body));
    }
    Ok((!inline.is_empty()).then(|| unescape_backslash_escapes(inline)))
}
