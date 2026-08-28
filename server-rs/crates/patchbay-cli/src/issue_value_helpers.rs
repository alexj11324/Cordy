use anyhow::{bail, Result};
use serde_json::Value;

const BUILT_IN_ISSUE_STATUSES: &[&str] = &[
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "blocked",
    "cancelled",
];
const ISSUE_PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

pub(super) fn format_metadata_value(value: Option<&Value>) -> String {
    match value.unwrap_or(&Value::Null) {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                value.to_string()
            } else if let Some(value) = value.as_u64() {
                value.to_string()
            } else if let Some(value) = value.as_f64() {
                if value.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    value.to_string()
                }
            } else {
                value.to_string()
            }
        }
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

pub(super) fn issue_labels(result: &Value) -> &[Value] {
    result
        .get("labels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(super) fn validate_issue_status(status: &str) -> Result<()> {
    let normalized = status.trim().to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    let valid = (1..=32).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_');
    if !valid {
        if normalized.is_empty() {
            bail!(
                "invalid status {status:?}; valid values: {}",
                BUILT_IN_ISSUE_STATUSES.join(", ")
            );
        }
        bail!(
            "invalid status {status:?}; a status key is 1-32 characters of lowercase letters, digits or underscore. Built-in values: {}",
            BUILT_IN_ISSUE_STATUSES.join(", ")
        );
    }
    Ok(())
}

pub(super) fn validate_issue_priority(priority: &str) -> Result<()> {
    if !ISSUE_PRIORITIES.contains(&priority) {
        bail!(
            "invalid priority {priority:?}; valid values: {}",
            ISSUE_PRIORITIES.join(", ")
        );
    }
    Ok(())
}
