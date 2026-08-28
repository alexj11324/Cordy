use anyhow::{bail, Result};
use serde_json::Value;

use super::{is_canonical_uuid, normalize_uuid_prefix, value_string, ApiClient};
pub(super) async fn resolve_issue_ref(client: &ApiClient, input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("issue id is required");
    }
    if looks_like_issue_identifier(trimmed) || is_canonical_uuid(trimmed) {
        let issue: Value = client.get_json(&format!("/api/issues/{trimmed}")).await?;
        return Ok(value_string(&issue, "id"));
    }
    if normalize_uuid_prefix(trimmed).is_some() {
        bail!(
            "issue ref {input:?} looks like a short UUID prefix; short prefixes are no longer supported for issues. Use the issue key (e.g. PB-123) shown by `cordy issue list`, or pass the full UUID (run a list command with --full-id to copy it)"
        );
    }
    bail!(
        "issue ref {input:?} is not a recognized issue reference; use the issue key (e.g. PB-123) shown by `cordy issue list`, or pass the full UUID"
    )
}

fn looks_like_issue_identifier(input: &str) -> bool {
    let Some((prefix, number)) = input.rsplit_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && number.trim().parse::<i64>().is_ok_and(|number| number > 0)
}
