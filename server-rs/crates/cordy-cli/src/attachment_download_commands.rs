use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;

use super::{http_timeout, new_api_client, value_string, Cli, Environment, RunOutput};

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
