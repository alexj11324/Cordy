use anyhow::{bail, Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{compact_uuid, is_canonical_uuid, normalize_uuid_prefix, value_string, ApiClient};
pub(super) async fn resolve_label_id(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<String> {
    resolve_label_reference(client, workspace_id, input)
        .await
        .map(|(id, _)| id)
}

pub(super) async fn resolve_label_reference(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<(String, String)> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok((trimmed.into(), trimmed.into()));
    }
    if workspace_id.is_empty() {
        bail!("resolve label: workspace_id is required to resolve label id prefixes");
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("resolve label: label id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve label: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve label: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let workspace = form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>();
    let result: Value = client
        .get_json(&format!("/api/labels?workspace_id={workspace}"))
        .await
        .context("resolve label")?;
    let mut matches = result
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|label| (value_string(label, "id"), value_string(label, "name")))
        .filter(|(id, _)| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [(id, display)] => Ok((
            id.clone(),
            if display.is_empty() {
                id.clone()
            } else {
                display.clone()
            },
        )),
        [] => bail!(
            "no label found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous label id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}
