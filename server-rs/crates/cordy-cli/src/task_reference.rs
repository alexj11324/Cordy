use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{compact_uuid, is_canonical_uuid, normalize_uuid_prefix, value_string, ApiClient};
pub(super) async fn resolve_task_run_id(
    client: &ApiClient,
    issue_id: Option<&str>,
    input: &str,
) -> Result<String> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok(trimmed.into());
    }
    let Some(issue_id) = issue_id.filter(|value| !value.trim().is_empty()) else {
        bail!(
            "short task run prefixes require --issue <issue-id>; pass a full task UUID or run `cordy issue runs <issue-id> --full-id`"
        );
    };
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("resolve task run: id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve task run: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve task run: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let runs: Vec<Value> = client
        .get_json(&format!("/api/issues/{issue_id}/task-runs"))
        .await
        .context("resolve task run")?;
    let mut matches = runs
        .iter()
        .map(|run| value_string(run, "id"))
        .filter(|id| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!(
            "no task run found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous task run id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches.join("\n  ")
        ),
    }
}
