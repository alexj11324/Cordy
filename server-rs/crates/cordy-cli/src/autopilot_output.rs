use std::collections::HashMap;

use serde_json::Value;

use super::{display_id, format_table, value_string};

pub(super) fn format_autopilot_runs_table(runs: &[Value]) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "SOURCE".into(),
        "STATUS".into(),
        "ISSUE".into(),
        "TRIGGERED_AT".into(),
        "COMPLETED_AT".into(),
    ]];
    rows.extend(runs.iter().map(|run| {
        vec![
            value_string(run, "id"),
            value_string(run, "source"),
            value_string(run, "status"),
            value_string(run, "issue_id"),
            value_string(run, "triggered_at"),
            value_string(run, "completed_at"),
        ]
    }));
    format_table(&rows)
}

pub(super) fn autopilot_webhook_url(trigger: &Value, base_url: &str) -> Option<String> {
    let url = value_string(trigger, "webhook_url");
    if !url.is_empty() {
        return Some(url);
    }
    let path = value_string(trigger, "webhook_path");
    (!path.is_empty()).then(|| format!("{}{path}", base_url.trim_end_matches('/')))
}

pub(super) fn format_autopilot_table(
    autopilots: &[Value],
    full_id: bool,
    agents: &HashMap<String, String>,
) -> String {
    let mut rows = vec![vec![
        "ID".into(),
        "TITLE".into(),
        "STATUS".into(),
        "MODE".into(),
        "ASSIGNEE".into(),
        "LAST_RUN".into(),
    ]];
    rows.extend(autopilots.iter().map(|autopilot| {
        let assignee_id = value_string(autopilot, "assignee_id");
        vec![
            display_id(&value_string(autopilot, "id"), full_id),
            value_string(autopilot, "title"),
            value_string(autopilot, "status"),
            value_string(autopilot, "execution_mode"),
            agents.get(&assignee_id).cloned().unwrap_or(assignee_id),
            value_string(autopilot, "last_run_at"),
        ]
    }));
    format_table(&rows)
}
