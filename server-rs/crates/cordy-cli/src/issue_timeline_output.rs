use serde_json::Value;

use super::{display_id, format_table, truncate_text, value_string, IssueActorNames};

fn timeline_actor(
    actor_type: &str,
    actor_id: &str,
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    match (actor_type.is_empty(), actor_id.is_empty()) {
        (true, true) => String::new(),
        (false, true) => actor_type.into(),
        (true, false) => display_id(actor_id, full_id),
        (false, false) => actors
            .0
            .get(&format!("{actor_type}:{actor_id}"))
            .map_or_else(
                || format!("{actor_type}:{}", display_id(actor_id, full_id)),
                |name| format!("{actor_type}:{name}"),
            ),
    }
}

fn timeline_transition(from: String, to: String) -> String {
    format!(
        "{} → {}",
        if from.is_empty() { "(none)" } else { &from },
        if to.is_empty() { "(none)" } else { &to }
    )
}

fn timeline_detail(entry: &Value, actors: &IssueActorNames, full_id: bool) -> String {
    if value_string(entry, "type") == "comment" {
        let content = value_string(entry, "content")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        return truncate_text(&content, 60);
    }
    let Some(details) = entry.get("details").and_then(Value::as_object) else {
        return String::new();
    };
    if details.contains_key("from") || details.contains_key("to") {
        return timeline_transition(
            value_string(&Value::Object(details.clone()), "from"),
            value_string(&Value::Object(details.clone()), "to"),
        );
    }
    if ["from_type", "from_id", "to_type", "to_id"]
        .iter()
        .any(|key| details.contains_key(*key))
    {
        let details = Value::Object(details.clone());
        return timeline_transition(
            timeline_actor(
                &value_string(&details, "from_type"),
                &value_string(&details, "from_id"),
                actors,
                full_id,
            ),
            timeline_actor(
                &value_string(&details, "to_type"),
                &value_string(&details, "to_id"),
                actors,
                full_id,
            ),
        );
    }
    let mut keys = details.keys().collect::<Vec<_>>();
    keys.sort();
    let text = keys
        .into_iter()
        .map(|key| {
            format!(
                "{key}={}",
                value_string(&Value::Object(details.clone()), key)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    truncate_text(&text, 60)
}

pub(super) fn format_issue_timeline_table(
    entries: &[Value],
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    let mut rows = vec![vec![
        "TIME".into(),
        "TYPE".into(),
        "ACTOR".into(),
        "DETAIL".into(),
    ]];
    rows.extend(entries.iter().map(|entry| {
        let action = value_string(entry, "action");
        vec![
            value_string(entry, "created_at").chars().take(16).collect(),
            if action.is_empty() {
                value_string(entry, "type")
            } else {
                action
            },
            timeline_actor(
                &value_string(entry, "actor_type"),
                &value_string(entry, "actor_id"),
                actors,
                full_id,
            ),
            timeline_detail(entry, actors, full_id),
        ]
    }));
    format_table(&rows)
}
