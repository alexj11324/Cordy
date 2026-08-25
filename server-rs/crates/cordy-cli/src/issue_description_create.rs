use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Read;
use std::path::Path;

use super::{
    ensure_file_within_workdir, trim_one_trailing_newline, unescape_backslash_escapes, Environment,
    IssueCreateArgs,
};

pub(super) fn resolve_issue_create_description<R: Read>(
    args: &IssueCreateArgs,
    environment: &Environment,
    input: &mut R,
) -> Result<Option<String>> {
    let inline = args.description.as_deref().unwrap_or_default();
    let description_file = args
        .description_file
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(Path::new);
    let sources = [
        args.description_stdin,
        !inline.is_empty(),
        description_file.is_some(),
    ]
    .into_iter()
    .filter(|source| *source)
    .count();
    if sources > 1 {
        bail!("--description, --description-stdin, and --description-file are mutually exclusive");
    }
    if args.description_stdin {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .context("read stdin for --description-stdin")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("stdin content for --description-stdin is empty");
        }
        return Ok(Some(body));
    }
    if let Some(path) = description_file {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "description",
        )?;
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            environment.current_dir().join(path)
        };
        let bytes = fs::read(path).context("read file for --description-file")?;
        let body = trim_one_trailing_newline(String::from_utf8_lossy(&bytes).into_owned());
        if body.is_empty() {
            bail!("file content for --description-file is empty");
        }
        return Ok(Some(body));
    }
    Ok((!inline.is_empty()).then(|| unescape_backslash_escapes(inline)))
}
