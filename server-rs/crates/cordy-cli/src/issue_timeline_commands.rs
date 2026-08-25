use anyhow::{bail, Context, Result};
use chrono::{DateTime, FixedOffset};
use serde_json::Value;
use std::collections::HashSet;

use super::{
    display_id, format_table, load_issue_actor_names, new_api_client, resolve_current_workspace_id,
    resolve_issue_ref, truncate_text, value_string, Cli, Environment, IssueActorNames,
    IssueTimelineArgs, OutputFormat, RunOutput,
};

#[derive(Debug)]
pub(super) struct TimelineFilter {
    activity_only: bool,
    actions: HashSet<String>,
    since: Option<DateTime<FixedOffset>>,
    tail: usize,
}

pub(super) fn build_timeline_filter(args: &IssueTimelineArgs) -> Result<TimelineFilter> {
    if args.tail < 0 {
        bail!("--tail must be >= 0");
    }
    let actions = args
        .action
        .iter()
        .map(|action| action.trim())
        .filter(|action| !action.is_empty())
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    let since = args
        .since
        .as_deref()
        .filter(|since| !since.is_empty())
        .map(|since| {
            DateTime::parse_from_rfc3339(since).with_context(|| {
                format!("invalid --since {since:?}: expected RFC3339, e.g. 2026-08-19T00:00:00Z")
            })
        })
        .transpose()?;
    Ok(TimelineFilter {
        activity_only: args.activity_only || !actions.is_empty(),
        actions,
        since,
        tail: args.tail as usize,
    })
}

pub(super) fn filter_timeline(entries: Vec<Value>, filter: &TimelineFilter) -> Vec<Value> {
    let mut entries = entries
        .into_iter()
        .filter(|entry| {
            if filter.activity_only && value_string(entry, "type") != "activity" {
                return false;
            }
            if !filter.actions.is_empty()
                && !filter.actions.contains(&value_string(entry, "action"))
            {
                return false;
            }
            let Some(since) = filter.since.as_ref() else {
                return true;
            };
            DateTime::parse_from_rfc3339(&value_string(entry, "created_at"))
                .is_ok_and(|created| created > *since)
        })
        .collect::<Vec<_>>();
    if filter.tail > 0 && entries.len() > filter.tail {
        entries.drain(..entries.len() - filter.tail);
    }
    entries
}

fn timeline_actor_inputs(entries: &[Value]) -> Vec<Value> {
    let mut actors = Vec::new();
    for entry in entries {
        actors.push(serde_json::json!({
            "assignee_type":entry.get("actor_type").cloned().unwrap_or(Value::Null),
            "assignee_id":entry.get("actor_id").cloned().unwrap_or(Value::Null),
        }));
        if let Some(details) = entry.get("details") {
            for prefix in ["from", "to"] {
                actors.push(serde_json::json!({
                    "assignee_type":details.get(format!("{prefix}_type")).cloned().unwrap_or(Value::Null),
                    "assignee_id":details.get(format!("{prefix}_id")).cloned().unwrap_or(Value::Null),
                }));
            }
        }
    }
    actors
}

pub(super) async fn run_issue_timeline(
    cli: &Cli,
    environment: &Environment,
    args: &IssueTimelineArgs,
) -> Result<RunOutput> {
    let filter = build_timeline_filter(args)?;
    let client = new_api_client(cli, environment)?;
    let issue_id = resolve_issue_ref(&client, &args.issue_id)
        .await
        .context("resolve issue")?;
    let (entries, headers) = client
        .get_json_with_headers::<Vec<Value>>(&format!("/api/issues/{issue_id}/timeline"))
        .await
        .context("list issue timeline")?;
    let entries = filter_timeline(entries, &filter);
    let truncated = headers
        .get("X-Timeline-Truncated")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let stderr = if truncated.is_empty() {
        String::new()
    } else {
        format!(
            "warning: timeline truncated by the server cap ({truncated}): older entries are missing. Durations and \"first entered <status>\" cannot be concluded from this read.\n"
        )
    };
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&entries)?),
        OutputFormat::Table => {
            let workspace_id = resolve_current_workspace_id(cli, environment);
            let actor_inputs = timeline_actor_inputs(&entries);
            let actors = load_issue_actor_names(&client, &workspace_id, &actor_inputs).await;
            format_issue_timeline_table(&entries, &actors, args.full_id)
        }
    };
    Ok(RunOutput { stdout, stderr })
}

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
