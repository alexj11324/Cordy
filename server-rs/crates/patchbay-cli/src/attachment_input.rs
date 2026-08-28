use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use super::{lexical_normalize, Environment};
#[derive(Debug)]
pub(super) struct PendingAttachment {
    pub(super) path: String,
    pub(super) data: Vec<u8>,
}

pub(super) fn append_unique_strings(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && seen.insert(value.to_string()) {
            output.push(value.into());
        }
    }
    output
}

pub(super) fn quick_create_attachment_ids(environment: &Environment) -> Result<Vec<String>> {
    let Some(raw) = environment
        .raw("PATCHBAY_QUICK_CREATE_ATTACHMENT_IDS")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> =
        serde_json::from_str(raw).context("parse PATCHBAY_QUICK_CREATE_ATTACHMENT_IDS")?;
    Ok(append_unique_strings(ids))
}

pub(super) fn collect_local_attachments(
    attachments: &[String],
    allow_external_file: bool,
    environment: &Environment,
) -> Result<(Vec<PendingAttachment>, String)> {
    let mut pending = Vec::with_capacity(attachments.len());
    let mut stderr = String::new();
    for file_path in attachments {
        let trimmed = file_path.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let _ = writeln!(
                stderr,
                "Skipping --attachment {file_path:?}: URLs are not supported here, only local file paths."
            );
            continue;
        }
        let path = Path::new(file_path);
        if !allow_external_file {
            let base = fs::canonicalize(environment.current_dir())
                .unwrap_or_else(|_| lexical_normalize(environment.current_dir()));
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                environment.current_dir().join(path)
            };
            let candidate =
                fs::canonicalize(&absolute).unwrap_or_else(|_| lexical_normalize(&absolute));
            if !candidate.starts_with(&base) {
                bail!(
                    "--attachment path {file_path:?} resolves outside the current working directory; attach files generated inside the task workdir rather than machine-shared paths like /tmp, where another run's stale file can be attached by mistake. Pass --allow-external-file to override."
                );
            }
        }
        let read_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let data = fs::read(read_path).with_context(|| format!("read attachment {file_path}"))?;
        pending.push(PendingAttachment {
            path: file_path.clone(),
            data,
        });
    }
    Ok((pending, stderr))
}
