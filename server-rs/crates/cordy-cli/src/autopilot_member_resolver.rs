//! Workspace member and agent resolution for autopilot commands.
//!
//! Matching precedence and member payload construction stay together here;
//! autopilot/trigger UUID lookup is kept in `autopilot_reference_resolver`.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use url::form_urlencoded;

use super::{
    display_id, is_canonical_uuid, normalize_assignee_input, retry_actor_get, value_string,
    ApiClient,
};

pub(super) async fn resolve_autopilot_agent(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<String> {
    if is_canonical_uuid(input) {
        return Ok(input.into());
    }
    if workspace_id.is_empty() {
        bail!(
            "workspace ID is required to resolve agents; use --workspace-id or set CORDY_WORKSPACE_ID"
        );
    }
    let path = format!(
        "/api/agents?workspace_id={}",
        form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
    );
    let agents: Vec<Value> = client.get_json(&path).await.context("fetch agents")?;
    let input_lower = input.to_ascii_lowercase();
    let matches = agents
        .iter()
        .filter(|agent| {
            let id = value_string(agent, "id");
            id.eq_ignore_ascii_case(input)
                || value_string(agent, "name")
                    .to_ascii_lowercase()
                    .contains(&input_lower)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [agent] => Ok(value_string(agent, "id")),
        [] => bail!("no agent found matching {input:?}"),
        agents => {
            let details = agents
                .iter()
                .map(|agent| {
                    format!(
                        "  {:?} ({})",
                        value_string(agent, "name"),
                        display_id(&value_string(agent, "id"), false)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("ambiguous agent {input:?}; matches:\n{details}")
        }
    }
}

pub(super) async fn resolve_autopilot_subscribers(
    client: &ApiClient,
    workspace_id: &str,
    refs: &[String],
) -> Result<Vec<Value>> {
    for raw in refs {
        if raw.trim().is_empty() {
            bail!("--subscriber cannot be empty");
        }
    }
    let path = format!("/api/workspaces/{workspace_id}/members");
    let members: Vec<Value> = retry_actor_get(client, &path).await.map_err(|error| {
        anyhow::anyhow!(
            "resolve subscriber {:?}: failed to resolve assignee: fetch members: {error:#}",
            refs.first().map(String::as_str).unwrap_or_default()
        )
    })?;
    let mut seen = HashSet::new();
    let mut subscribers = Vec::new();
    for raw in refs {
        let input = normalize_assignee_input(raw);
        let input_lower = input.to_ascii_lowercase();
        let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
        for member in &members {
            let id = value_string(member, "user_id");
            let name = value_string(member, "name");
            let email = value_string(member, "email");
            if id.eq_ignore_ascii_case(&input)
                || display_id(&id, false).eq_ignore_ascii_case(&input)
                || (!email.is_empty() && email.eq_ignore_ascii_case(&input))
            {
                buckets[0].push(member);
            } else if name.eq_ignore_ascii_case(&input) {
                buckets[1].push(member);
            } else if name.to_ascii_lowercase().contains(&input_lower) {
                buckets[2].push(member);
            }
        }
        let member = buckets
            .iter()
            .find(|bucket| !bucket.is_empty())
            .ok_or_else(|| {
                let missing = if input.is_empty() {
                    raw.as_str()
                } else {
                    input.as_str()
                };
                anyhow::anyhow!("resolve subscriber {raw:?}: no member found matching {missing:?}")
            })?;
        if member.len() > 1 {
            let details = member
                .iter()
                .map(|member| {
                    format!(
                        "  member {:?} ({})",
                        value_string(member, "name"),
                        display_id(&value_string(member, "user_id"), false)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("resolve subscriber {raw:?}: ambiguous assignee {input:?}; matches:\n{details}");
        }
        let user_id = value_string(member[0], "user_id");
        if seen.insert(user_id.clone()) {
            subscribers.push(serde_json::json!({"user_type":"member","user_id":user_id}));
        }
    }
    Ok(subscribers)
}

pub(super) async fn load_autopilot_agent_names(
    client: &ApiClient,
    workspace_id: &str,
    autopilots: &[Value],
) -> HashMap<String, String> {
    if workspace_id.is_empty()
        || !autopilots
            .iter()
            .any(|autopilot| !value_string(autopilot, "assignee_id").is_empty())
    {
        return HashMap::new();
    }
    let path = format!(
        "/api/agents?workspace_id={}",
        form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
    );
    let Ok(agents) = client.get_json::<Vec<Value>>(&path).await else {
        return HashMap::new();
    };
    agents
        .into_iter()
        .filter_map(|agent| {
            let id = value_string(&agent, "id");
            let name = value_string(&agent, "name");
            (!id.is_empty() && !name.is_empty()).then_some((id, name))
        })
        .collect()
}
