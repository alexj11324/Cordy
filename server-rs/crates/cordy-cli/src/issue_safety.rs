use anyhow::{bail, Result};
use serde_json::Value;
use std::fmt::Write;

use super::{issue_markdown_links::find_runtime_local_markdown_links, Environment, HttpError};

pub(super) fn active_duplicate_issue_message(error: &anyhow::Error) -> Option<String> {
    let error = error.downcast_ref::<HttpError>()?;
    if error.status_code != 409 {
        return None;
    }
    let payload: Value = serde_json::from_str(&error.body).ok()?;
    (payload.get("code").and_then(Value::as_str) == Some("active_duplicate_issue"))
        .then(|| {
            payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .filter(|message| !message.is_empty())
}

pub(super) fn guard_issue_description_local_links(
    description: &str,
    environment: &Environment,
    remediation: &str,
) -> Result<()> {
    if !environment.in_agent_execution_context() {
        return Ok(());
    }
    let findings = find_runtime_local_markdown_links(description, environment.current_dir());
    if findings.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "issue description links {} runtime-local path(s), which no reader can open:\n",
        findings.len()
    );
    for (target, reason) in findings {
        let _ = writeln!(message, "  - {target:?} — {reason}");
    }
    message.push_str(
        "\nThe path exists only on the machine running you; for everyone else the link is dead. ",
    );
    message.push_str(remediation);
    message.push_str("\nTo merely reference a code location, use inline code instead of a link (`path/to/file.ts:42`) — code spans and fenced blocks are not checked.");
    bail!("{message}")
}
