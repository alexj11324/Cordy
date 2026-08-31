//! Patrick-only issue mutations.
//!
//! The server remains the authority for the Patrick identity, task binding,
//! revision guard, field allowlist, Linear remote version check, and audit
//! record. The CLI only performs safe input loading and forwards the exact
//! caller-supplied correlation metadata.

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::fs;
use std::io::Read;
use uuid::Uuid;

use super::{
    ensure_file_within_workdir, format_table, new_api_client, resolve_issue_ref, value_string, Cli,
    Environment, IssuePatrickMutationArgs, OutputFormat, RunOutput,
};

pub(super) async fn run_issue_patrick_mutation<R: Read>(
    cli: &Cli,
    environment: &Environment,
    args: &IssuePatrickMutationArgs,
    input: &mut R,
) -> Result<RunOutput> {
    if args.expected_revision <= 0 {
        bail!("--expected-revision must be positive");
    }
    if args.change_reason.trim().is_empty() {
        bail!("--change-reason must not be empty");
    }
    let correlation_id = Uuid::parse_str(args.correlation_id.trim())
        .context("--correlation-id must be a UUID")?;
    let changes = read_changes(environment, args, input)?;
    if changes.is_empty() {
        bail!("the Patrick mutation changes object must not be empty");
    }

    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.id)
        .await
        .context("resolve issue")?;
    let mut body = Map::new();
    body.insert(
        "expected_revision".into(),
        Value::Number(args.expected_revision.into()),
    );
    body.insert(
        "change_reason".into(),
        Value::String(args.change_reason.trim().to_string()),
    );
    body.insert(
        "correlation_id".into(),
        Value::String(correlation_id.to_string()),
    );
    body.insert("changes".into(), Value::Object(changes));
    if let Some(value) = &args.task_id {
        body.insert("task_id".into(), Value::String(value.clone()));
    }
    if let Some(value) = &args.run_id {
        body.insert("run_id".into(), Value::String(value.clone()));
    }
    if let Some(value) = &args.linear_remote_updated_at {
        body.insert(
            "linear_remote_updated_at".into(),
            Value::String(value.clone()),
        );
    }
    if let Some(value) = &args.linear_remote_snapshot {
        let snapshot: Value = serde_json::from_str(value)
            .context("--linear-remote-snapshot must be valid JSON")?;
        if !snapshot.is_object() {
            bail!("--linear-remote-snapshot must be a JSON object");
        }
        body.insert("linear_remote_snapshot".into(), snapshot);
    }

    let issue: Value = client
        .post_json(&format!("/api/issues/{issue_id}/patrick-mutation"), &body)
        .await
        .context("apply Patrick issue mutation")?;
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&issue)?),
        OutputFormat::Table => format_table(&[
            vec!["KEY".into(), "TITLE".into(), "REVISION".into()],
            vec![
                value_string(&issue, "identifier"),
                value_string(&issue, "title"),
                value_string(&issue, "revision"),
            ],
        ]),
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn read_changes<R: Read>(
    environment: &Environment,
    args: &IssuePatrickMutationArgs,
    input: &mut R,
) -> Result<Map<String, Value>> {
    let text = if args.changes_stdin {
        let mut text = String::new();
        input
            .read_to_string(&mut text)
            .context("read Patrick mutation changes from stdin")?;
        text
    } else if let Some(text) = &args.changes_json {
        text.clone()
    } else if let Some(path) = args.changes_file.as_deref() {
        ensure_file_within_workdir(
            path,
            environment.current_dir(),
            args.allow_external_file,
            "Patrick mutation changes",
        )?;
        fs::read_to_string(path).with_context(|| format!("read Patrick mutation changes {:?}", path))?
    } else {
        bail!("one of --changes-json, --changes-file, or --changes-stdin is required");
    };
    let value: Value = serde_json::from_str(&text).context("parse Patrick mutation changes JSON")?;
    value
        .as_object()
        .cloned()
        .context("Patrick mutation changes must be a JSON object")
}
