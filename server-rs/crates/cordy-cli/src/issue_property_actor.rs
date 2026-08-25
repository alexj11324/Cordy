use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::{display_id, is_canonical_uuid, normalize_assignee_input, retry_actor_get, ApiClient};

pub(super) async fn resolve_property_member(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<String> {
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required to resolve assignees; use --workspace-id or set CORDY_WORKSPACE_ID"
        );
    }
    let token = raw.trim();
    if let Some(id) = token.strip_prefix("member:") {
        let id = id.trim();
        if !is_canonical_uuid(id) {
            bail!("actor id in {token:?} must be a UUID");
        }
        return Ok(format!("member:{id}"));
    }
    let input = normalize_assignee_input(token);
    if input.is_empty() {
        bail!("actor value cannot be empty");
    }
    let members =
        retry_actor_get::<Vec<Value>>(client, &format!("/api/workspaces/{workspace_id}/members"))
            .await
            .context("fetch members")?;
    let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
    for member in &members {
        let id = super::value_string(member, "user_id");
        let name = super::value_string(member, "name");
        let email = super::value_string(member, "email");
        if id.eq_ignore_ascii_case(&input)
            || display_id(&id, false).eq_ignore_ascii_case(&input)
            || (!email.is_empty() && email.eq_ignore_ascii_case(&input))
        {
            buckets[0].push((id, name));
        } else if name.eq_ignore_ascii_case(&input) {
            buckets[1].push((id, name));
        } else if name
            .to_ascii_lowercase()
            .contains(&input.to_ascii_lowercase())
        {
            buckets[2].push((id, name));
        }
    }
    for bucket in buckets {
        match bucket.as_slice() {
            [] => {}
            [(id, _)] => return Ok(format!("member:{id}")),
            matches => {
                let matches = matches
                    .iter()
                    .map(|(id, name)| format!("  member {name:?} ({})", display_id(id, false)))
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!("ambiguous assignee {input:?}; matches:\n{matches}");
            }
        }
    }
    bail!("no member found matching {input:?}")
}
