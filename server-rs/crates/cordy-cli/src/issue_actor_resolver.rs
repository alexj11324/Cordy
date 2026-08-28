use super::api::NetworkError;
use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::form_urlencoded;

use super::{compact_uuid, display_id, is_canonical_uuid, value_string, ApiClient};
#[derive(Clone, Debug)]
struct IssueActor {
    actor_type: &'static str,
    id: String,
    name: String,
    email: String,
    archived: bool,
}

#[derive(Debug)]
pub(super) struct ResolvedIssueAssignee {
    pub(super) actor_type: String,
    pub(super) id: String,
    pub(super) name: String,
}

async fn fetch_issue_actors(
    client: &ApiClient,
    workspace_id: &str,
    include_squads: bool,
) -> [Result<Vec<IssueActor>>; 3] {
    let members =
        retry_actor_get::<Vec<Value>>(client, &format!("/api/workspaces/{workspace_id}/members"))
            .await
            .map(|items| {
                items
                    .iter()
                    .map(|item| IssueActor {
                        actor_type: "member",
                        id: value_string(item, "user_id"),
                        name: value_string(item, "name"),
                        email: value_string(item, "email"),
                        archived: false,
                    })
                    .collect()
            });
    let agents = retry_actor_get::<Vec<Value>>(
        client,
        &format!(
            "/api/agents?workspace_id={}",
            form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
        ),
    )
    .await
    .map(|items| {
        items
            .iter()
            .map(|item| IssueActor {
                actor_type: "agent",
                id: value_string(item, "id"),
                name: value_string(item, "name"),
                email: String::new(),
                archived: false,
            })
            .collect()
    });
    let squads = if include_squads {
        retry_actor_get::<Vec<Value>>(client, "/api/squads")
            .await
            .map(|items| {
                items
                    .iter()
                    .map(|item| IssueActor {
                        actor_type: "squad",
                        id: value_string(item, "id"),
                        name: value_string(item, "name"),
                        email: String::new(),
                        archived: !value_string(item, "archived_at").is_empty(),
                    })
                    .collect()
            })
    } else {
        Ok(Vec::new())
    };
    [members, agents, squads]
}

pub(super) async fn retry_actor_get<T: DeserializeOwned>(
    client: &ApiClient,
    path: &str,
) -> Result<T> {
    let delays = [100_u64, 250];
    for (attempt, delay) in [0_u64, 100, 250].into_iter().enumerate() {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        match client.get_json(path).await {
            Ok(value) => return Ok(value),
            Err(error)
                if error.downcast_ref::<NetworkError>().is_some() && attempt < delays.len() => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("actor resolver retry loop always returns")
}

pub(super) async fn resolve_issue_assignee_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_id(client, workspace_id, raw, true).await
}

pub(super) async fn resolve_subscriber_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_id(client, workspace_id, raw, false).await
}

async fn resolve_actor_id(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
    allow_squads: bool,
) -> Result<ResolvedIssueAssignee> {
    let input = raw.trim();
    if !is_canonical_uuid(input) {
        bail!("expected a canonical UUID, got {raw:?}");
    }
    let actors = fetch_issue_actors(client, workspace_id, allow_squads).await;
    let actor_kind_count = if allow_squads { 3 } else { 2 };
    if actors[..actor_kind_count].iter().all(Result::is_err) {
        let errors = actors[..actor_kind_count]
            .iter()
            .enumerate()
            .map(|(index, result)| {
                let kind = ["members", "agents", "squads"][index];
                format!("fetch {kind}: {}", result.as_ref().unwrap_err())
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !allow_squads {
            bail!("failed to resolve user: {errors}");
        }
        bail!(
            "failed to resolve assignee: {}; {}; {}",
            actors[0].as_ref().unwrap_err(),
            actors[1].as_ref().unwrap_err(),
            actors[2].as_ref().unwrap_err()
        );
    }
    if let Some(actor) = actors
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flatten()
        .find(|actor| {
            (allow_squads || actor.actor_type != "squad") && actor.id.eq_ignore_ascii_case(input)
        })
    {
        return Ok(ResolvedIssueAssignee {
            actor_type: actor.actor_type.into(),
            id: actor.id.clone(),
            name: actor.name.clone(),
        });
    }
    if allow_squads {
        bail!("no member, agent, or squad found with ID {input:?}")
    }
    bail!("no member or agent found with ID {input:?}")
}

pub(super) async fn resolve_issue_assignee_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_name(client, workspace_id, raw, true).await
}

pub(super) async fn resolve_subscriber_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
) -> Result<ResolvedIssueAssignee> {
    resolve_actor_name(client, workspace_id, raw, false).await
}

async fn resolve_actor_name(
    client: &ApiClient,
    workspace_id: &str,
    raw: &str,
    allow_squads: bool,
) -> Result<ResolvedIssueAssignee> {
    let input = normalize_assignee_input(raw);
    if input.is_empty() {
        if allow_squads {
            bail!("no member, agent, or squad found matching {raw:?}");
        }
        bail!("no member or agent found matching {raw:?}");
    }
    let actors = fetch_issue_actors(client, workspace_id, allow_squads).await;
    let actor_kind_count = if allow_squads { 3 } else { 2 };
    if actors[..actor_kind_count].iter().all(Result::is_err) {
        let errors = actors[..actor_kind_count]
            .iter()
            .enumerate()
            .map(|(index, result)| {
                let kind = ["members", "agents", "squads"][index];
                format!("fetch {kind}: {}", result.as_ref().unwrap_err())
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !allow_squads {
            bail!("failed to resolve user: {errors}");
        }
        bail!("failed to resolve assignee: {errors}");
    }
    let actors = actors
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .flatten()
        .filter(|actor| !actor.archived && (allow_squads || actor.actor_type != "squad"))
        .collect::<Vec<_>>();
    let mut buckets = [Vec::new(), Vec::new(), Vec::new()];
    for actor in actors {
        let short_id = display_id(&actor.id, false);
        if actor.id.eq_ignore_ascii_case(&input)
            || short_id.eq_ignore_ascii_case(&input)
            || (!actor.email.is_empty() && actor.email.eq_ignore_ascii_case(&input))
        {
            buckets[0].push(actor);
        } else if actor.name.eq_ignore_ascii_case(&input) {
            buckets[1].push(actor);
        } else if actor
            .name
            .to_ascii_lowercase()
            .contains(&input.to_ascii_lowercase())
        {
            buckets[2].push(actor);
        }
    }
    for bucket in buckets {
        match bucket.as_slice() {
            [] => {}
            [actor] => {
                return Ok(ResolvedIssueAssignee {
                    actor_type: actor.actor_type.into(),
                    id: actor.id.clone(),
                    name: actor.name.clone(),
                });
            }
            actors => {
                let matches = actors
                    .iter()
                    .map(|actor| {
                        format!(
                            "  {} {:?} ({})",
                            actor.actor_type,
                            actor.name,
                            display_id(&actor.id, false)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!("ambiguous assignee {input:?}; matches:\n{matches}");
            }
        }
    }
    if allow_squads {
        bail!("no member, agent, or squad found matching {input:?}")
    }
    bail!("no member or agent found matching {input:?}")
}

pub(super) fn normalize_assignee_input(raw: &str) -> String {
    let input = raw.trim();
    if let Some(marker) = input.find("](mention://") {
        if input.starts_with('[') && input.ends_with(')') {
            let target = &input[marker + 12..input.len() - 1];
            if let Some((kind, id)) = target.split_once('/') {
                if matches!(kind, "member" | "agent" | "squad") {
                    return id.into();
                }
            }
        }
    }
    input.trim_start_matches(['@', '＠']).trim().to_string()
}

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
