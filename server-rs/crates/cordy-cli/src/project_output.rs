//! Project table rendering and actor-input preparation.
//!
//! Project API reads remain in `project_commands`; this module keeps display
//! and actor enrichment inputs independent from request orchestration.

use serde_json::Value;

use super::{display_id, format_table, value_string, IssueActorNames};

pub(super) fn project_lead(project: &Value, actors: &IssueActorNames) -> String {
    let actor_type = value_string(project, "lead_type");
    let actor_id = value_string(project, "lead_id");
    if actor_type.is_empty() || actor_id.is_empty() {
        return String::new();
    }
    let key = format!("{actor_type}:{actor_id}");
    actors
        .0
        .get(&key)
        .map_or(key, |name| format!("{actor_type}:{name}"))
}

pub(super) fn project_actor_inputs(projects: &[Value]) -> Vec<Value> {
    projects
        .iter()
        .map(|project| {
            serde_json::json!({
                "assignee_type":project.get("lead_type").cloned().unwrap_or(Value::Null),
                "assignee_id":project.get("lead_id").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

pub(super) fn format_project_list_table(
    projects: &[Value],
    actors: &IssueActorNames,
    full_id: bool,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TITLE".into(),
        "STATUS".into(),
        "LEAD".into(),
        "CREATED".into(),
    ]];
    rows.extend(projects.iter().map(|project| {
        vec![
            display_id(&value_string(project, "id"), full_id),
            value_string(project, "title"),
            value_string(project, "status"),
            project_lead(project, actors),
            value_string(project, "created_at")
                .chars()
                .take(10)
                .collect(),
        ]
    }));
    format_table(&rows)
}

pub(super) fn format_project_details_table(project: &Value, actors: &IssueActorNames) -> String {
    format_table(&[
        vec![
            "ID".into(),
            "TITLE".into(),
            "STATUS".into(),
            "LEAD".into(),
            "DESCRIPTION".into(),
        ],
        vec![
            value_string(project, "id"),
            value_string(project, "title"),
            value_string(project, "status"),
            project_lead(project, actors),
            value_string(project, "description"),
        ],
    ])
}
