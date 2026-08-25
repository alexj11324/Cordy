use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{compact_uuid, is_canonical_uuid, normalize_uuid_prefix, value_string, ApiClient};

pub(super) fn project_resources(result: &Value) -> &[Value] {
    result
        .get("resources")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(super) async fn resolve_project_resource_reference(
    client: &ApiClient,
    project_id: &str,
    raw: &str,
) -> Result<(String, String)> {
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok((input.into(), input.into()));
    }
    let Some(prefix) = normalize_uuid_prefix(input) else {
        if input.is_empty() {
            bail!("resolve project resource: project resource id is required");
        }
        let compact = input.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve project resource: expected a full UUID or at least 4 hex characters, got {raw:?}"
            );
        }
        bail!(
            "resolve project resource: expected a UUID prefix containing only hex characters, got {raw:?}"
        );
    };
    let result: Value = client
        .get_json(&format!("/api/projects/{project_id}/resources"))
        .await
        .context("resolve project resource")?;
    let mut matches = project_resources(&result)
        .iter()
        .filter_map(|resource| {
            let id = value_string(resource, "id");
            if id.is_empty() || !compact_uuid(&id).starts_with(&prefix) {
                return None;
            }
            let label = value_string(resource, "label");
            let resource_type = value_string(resource, "resource_type");
            Some((
                id.clone(),
                if label.is_empty() {
                    if resource_type.is_empty() {
                        id
                    } else {
                        resource_type
                    }
                } else {
                    label
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [(id, display)] => Ok((id.clone(), display.clone())),
        [] => bail!(
            "no project resource found matching id prefix {raw:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous project resource id prefix {raw:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
}

pub(super) fn find_project_resource<'a>(
    resources: &'a [Value],
    resource_id: &str,
) -> Option<&'a Value> {
    resources
        .iter()
        .find(|resource| value_string(resource, "id") == resource_id)
}
