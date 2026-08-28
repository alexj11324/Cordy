use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use url::form_urlencoded;

use super::{
    compact_uuid, display_id, is_canonical_uuid, normalize_assignee_input, normalize_uuid_prefix,
    retry_actor_get, value_string, ApiClient,
};

#[derive(Debug, Deserialize)]
struct AutopilotResolverEnvelope {
    autopilots: Vec<Value>,
    #[serde(default)]
    total: i64,
    #[serde(default)]
    has_more: bool,
}

pub(super) async fn resolve_autopilot_trigger_id(
    client: &ApiClient,
    autopilot_id: &str,
    input: &str,
) -> Result<String> {
    let trimmed = input.trim();
    if is_canonical_uuid(trimmed) {
        return Ok(trimmed.into());
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        if trimmed.is_empty() {
            bail!("autopilot trigger id is required");
        }
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve autopilot trigger: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve autopilot trigger: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    let response: Value = client
        .get_json(&format!("/api/autopilots/{autopilot_id}"))
        .await
        .map_err(|error| anyhow::anyhow!("resolve autopilot trigger: {error:#}"))?;
    let mut matches = response
        .get("triggers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|trigger| value_string(trigger, "id"))
        .filter(|id| !id.is_empty() && compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!(
            "no autopilot trigger found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous autopilot trigger id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches.join("\n  ")
        ),
    }
}

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
            let name = value_string(agent, "name");
            id.eq_ignore_ascii_case(input)
                || display_id(&id, false).eq_ignore_ascii_case(input)
                || name.to_ascii_lowercase().contains(&input_lower)
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

pub(super) async fn resolve_autopilot_id(
    client: &ApiClient,
    workspace_id: &str,
    input: &str,
) -> Result<(String, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("autopilot id is required");
    }
    if is_canonical_uuid(trimmed) {
        return Ok((trimmed.into(), trimmed.into()));
    }
    let Some(prefix) = normalize_uuid_prefix(trimmed) else {
        let compact = trimmed.replace('-', "");
        if compact.len() < 4 {
            bail!(
                "resolve autopilot: expected a full UUID or at least 4 hex characters, got {input:?}"
            );
        }
        bail!(
            "resolve autopilot: expected a UUID prefix containing only hex characters, got {input:?}"
        );
    };
    if workspace_id.is_empty() {
        bail!("resolve autopilot: workspace_id is required to resolve autopilot id prefixes");
    }

    const LIMIT: usize = 50;
    let mut offset = 0;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    loop {
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("limit", &LIMIT.to_string());
        if offset > 0 {
            query.append_pair("offset", &offset.to_string());
        }
        query.append_pair("workspace_id", workspace_id);
        let page: AutopilotResolverEnvelope = client
            .get_json(&format!("/api/autopilots?{}", query.finish()))
            .await
            .map_err(|error| anyhow::anyhow!("resolve autopilot: {error:#}"))?;
        let page_len = page.autopilots.len();
        let mut added = 0;
        for autopilot in page.autopilots {
            let id = value_string(&autopilot, "id");
            if !id.is_empty() && seen.insert(id.clone()) {
                added += 1;
                let title = value_string(&autopilot, "title");
                candidates.push((id.clone(), if title.is_empty() { id } else { title }));
            }
        }
        offset += page_len;
        if page_len == 0 || added == 0 || page_len < LIMIT {
            break;
        }
        if page.has_more {
            continue;
        }
        if page.total > 0 {
            if offset as i64 >= page.total {
                break;
            }
            continue;
        }
        break;
    }

    let mut matches = candidates
        .into_iter()
        .filter(|(id, _)| compact_uuid(id).starts_with(&prefix))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    match matches.as_slice() {
        [resolved] => Ok(resolved.clone()),
        [] => bail!(
            "no autopilot found matching id prefix {input:?}; run the list command with --full-id to copy the full UUID"
        ),
        _ => bail!(
            "ambiguous autopilot id prefix {input:?}; matches:\n  {}\nUse more characters or run the list command with --full-id",
            matches
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        ),
    }
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
