use serde_json::Value;

use super::{display_id, format_metadata_value, format_table, value_string, IssueActorNames};

pub(super) fn format_issue_runs_table(
    runs: &[Value],
    full_id: bool,
    actors: &IssueActorNames,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "AGENT".into(),
        "STATUS".into(),
        "STARTED".into(),
        "COMPLETED".into(),
        "ERROR".into(),
    ]];
    for run in runs {
        let agent_id = value_string(run, "agent_id");
        let agent = actors
            .0
            .get(&format!("agent:{agent_id}"))
            .cloned()
            .unwrap_or(agent_id);
        let error = value_string(run, "error");
        let error = if error.chars().count() > 50 {
            format!("{}...", error.chars().take(47).collect::<String>())
        } else {
            error
        };
        let timestamp = |field| {
            value_string(run, field)
                .chars()
                .take(16)
                .collect::<String>()
        };
        rows.push(vec![
            display_id(&value_string(run, "id"), full_id),
            agent,
            value_string(run, "status"),
            timestamp("started_at"),
            timestamp("completed_at"),
            error,
        ]);
    }
    format_table(&rows)
}

pub(super) fn format_issue_run_messages_table(messages: &[Value]) -> String {
    let mut rows = vec![vec![
        "SEQ".into(),
        "TYPE".into(),
        "TOOL".into(),
        "CONTENT".into(),
    ]];
    for message in messages {
        let mut content = value_string(message, "content");
        if content.is_empty() {
            content = value_string(message, "output");
        }
        if content.chars().count() > 80 {
            content = format!("{}...", content.chars().take(77).collect::<String>());
        }
        rows.push(vec![
            message
                .get("seq")
                .map(|value| format_metadata_value(Some(value)))
                .unwrap_or_default(),
            value_string(message, "type"),
            value_string(message, "tool"),
            content,
        ]);
    }
    format_table(&rows)
}
