use anyhow::{bail, Context, Result};
use serde_json::Value;
use url::form_urlencoded;

use super::{compact_uuid, is_canonical_uuid, value_string, ApiClient};

pub(super) async fn resolve_issue_project_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<String> {
    resolve_project_reference(client, workspace_id, raw)
        .await
        .map(|(id, _)| id)
}

pub(super) async fn resolve_project_reference(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<(String, String)> {
    let input = raw.trim();
    if is_canonical_uuid(input) {
        return Ok((input.into(), input.into()));
    }
    let compact = input.replace('-', "").to_ascii_lowercase();
    if compact.len() < 4 {
        bail!("resolve project: expected a full UUID or at least 4 hex characters, got {raw:?}");
    }
    if !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!(
            "resolve project: expected a UUID prefix containing only hex characters, got {raw:?}"
        );
    }
    let path = format!(
        "/api/projects?workspace_id={}",
        form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
    );
    let result: Value = client.get_json(&path).await.context("resolve project")?;
    let mut candidates = result
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|project| compact_uuid(&value_string(project, "id")).starts_with(&compact))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|project| value_string(project, "id"));
    match candidates.as_slice() {
        [project] => {
            let id = value_string(project, "id");
            let title = value_string(project, "title");
            Ok((id.clone(), if title.is_empty() { id } else { title }))
        }
        [] => bail!(
            "no project found matching id prefix {raw:?}; run the list command with --full-id to copy the full UUID"
        ),
        projects => {
            let matches = projects
                .iter()
                .map(|project| format!("  {}", value_string(project, "id")))
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "ambiguous project id prefix {raw:?}; matches:\n{matches}\nUse more characters or run the list command with --full-id"
            )
        }
    }
}
