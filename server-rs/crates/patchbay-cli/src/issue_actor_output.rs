use serde_json::Value;
use std::collections::HashMap;
use url::form_urlencoded;

use super::{format_table, value_string, ApiClient};
#[derive(Debug, Default)]
pub(super) struct IssueActorNames(pub(super) HashMap<String, String>);

pub(super) async fn load_issue_actor_names(
    client: &ApiClient,
    workspace_id: &str,
    issues: &[Value],
) -> IssueActorNames {
    let needed = issues
        .iter()
        .filter_map(|issue| issue.get("assignee_type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    if needed.is_empty() || workspace_id.is_empty() {
        return IssueActorNames::default();
    }
    let mut names = HashMap::new();
    let paths = [
        (
            "member",
            format!("/api/workspaces/{workspace_id}/members"),
            "user_id",
        ),
        (
            "agent",
            format!(
                "/api/agents?workspace_id={}",
                form_urlencoded::byte_serialize(workspace_id.as_bytes()).collect::<String>()
            ),
            "id",
        ),
        ("team", "/api/teams".into(), "id"),
    ];
    for (actor_type, path, id_field) in paths {
        if !needed.contains(&actor_type) {
            continue;
        }
        if let Ok(items) = client.get_json::<Vec<Value>>(&path).await {
            for item in items {
                let id = value_string(&item, id_field);
                let name = value_string(&item, "name");
                if !id.is_empty() && !name.is_empty() {
                    names.insert(format!("{actor_type}:{id}"), name);
                }
            }
        }
    }
    IssueActorNames(names)
}

pub(super) fn format_issue_list_table(
    issues: &[Value],
    full_id: bool,
    actors: &IssueActorNames,
) -> String {
    let mut rows = Vec::with_capacity(issues.len() + 1);
    let mut headers = vec![
        "KEY".into(),
        "TITLE".into(),
        "STATUS".into(),
        "PRIORITY".into(),
        "ASSIGNEE".into(),
        "START DATE".into(),
        "DUE DATE".into(),
    ];
    if full_id {
        headers.insert(1, "ID".into());
    }
    rows.push(headers);
    for issue in issues {
        let id = value_string(issue, "id");
        let key = match value_string(issue, "identifier") {
            value if value.is_empty() => id.clone(),
            value => value,
        };
        let actor_type = value_string(issue, "assignee_type");
        let actor_id = value_string(issue, "assignee_id");
        let assignee = if actor_type.is_empty() || actor_id.is_empty() {
            String::new()
        } else {
            let actor_key = format!("{actor_type}:{actor_id}");
            actors
                .0
                .get(&actor_key)
                .map_or_else(|| actor_key.clone(), |name| format!("{actor_type}:{name}"))
        };
        let date = |field| {
            value_string(issue, field)
                .chars()
                .take(10)
                .collect::<String>()
        };
        let mut row = vec![
            key,
            value_string(issue, "title"),
            value_string(issue, "status"),
            value_string(issue, "priority"),
            assignee,
            date("start_date"),
            date("due_date"),
        ];
        if full_id {
            row.insert(1, id);
        }
        rows.push(row);
    }
    format_table(&rows)
}
