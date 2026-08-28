//! Task wire-shape builder — port of Go `taskToResponse`
//! Builds the task JSON map. Key names and
//! null-vs-absent behavior match the Go struct tags byte-for-byte so clients
//! type both shapes identically.

use serde_json::{json, Map, Value};

use patchbay_db::models::AgentTaskQueue;

/// Go relativeWorkDir: a privacy-safe display form of the daemon-reported
/// absolute work_dir. Never contains the user's home prefix or account name.
fn task_dir_segment(uuid: &str) -> String {
    let s = uuid.replace('-', "");
    const SEGMENT_LEN: usize = 12;
    if s.len() > SEGMENT_LEN {
        s[s.len() - SEGMENT_LEN..].to_string()
    } else {
        s
    }
}

fn strip_home_prefix(p: &str) -> Option<String> {
    // Case-insensitive `(?:[A-Za-z]:)?/(?:Users|home)/[^/]+(?:/(.*))?`.
    let normalized = p.replace('\\', "/");
    let lower = normalized.to_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("/users/") {
        r
    } else if let Some(r) = lower.strip_prefix("/home/") {
        r
    } else {
        // Windows drive form `c:/users/...`
        let bytes = lower.as_bytes();
        if bytes.len() > 8 && bytes[1] == b':' && lower[2..].starts_with("/users/") {
            &lower[8..]
        } else {
            return None;
        }
    };
    let remainder = match rest.find('/') {
        Some(idx) => &normalized[normalized.len() - (rest.len() - idx)..],
        None => "",
    };
    Some(remainder.to_string())
}

fn basename(p: &str) -> String {
    let p = p.trim_end_matches('/');
    match p.rfind('/') {
        Some(idx) => p[idx + 1..].to_string(),
        None => p.to_string(),
    }
}

pub fn relative_work_dir(work_dir: &str, workspace_id: &str, task_id: &str) -> String {
    if work_dir.is_empty() {
        return String::new();
    }
    let normalized = work_dir.replace('\\', "/");
    if !workspace_id.is_empty() && !task_id.is_empty() {
        let suffix = format!("{}/{}", workspace_id, task_dir_segment(task_id));
        if let Some(idx) = normalized.find(&suffix) {
            return normalized[idx..].to_string();
        }
    }
    if let Some(stripped) = strip_home_prefix(&normalized) {
        return stripped;
    }
    basename(&normalized)
}

/// Pure attribution labels from the row (Go taskAttributionBase). Names are
/// hydrated separately on user-facing surfaces.
fn attribution_base(t: &AgentTaskQueue) -> Value {
    let source = t.originator_source.clone().unwrap_or_default();
    let precise = matches!(
        source.as_str(),
        "direct_human" | "delegation" | "comment_source" | "trigger_owner" | "rule_owner"
    );
    let mut value = Map::new();
    value.insert(
        "source".into(),
        Value::String(if source.is_empty() {
            "unattributed".into()
        } else {
            source
        }),
    );
    value.insert("precise".into(), Value::Bool(precise));

    if let Some(id) = t.accountable_user_id {
        value.insert("initiator".into(), json!({ "id": id.to_string() }));
    }
    if let Some(id) = t.originator_user_id {
        value.insert("originator".into(), json!({ "id": id.to_string() }));
    }
    if let Some(kind) = t.trigger_evidence_kind.as_deref().filter(|k| !k.is_empty()) {
        value.insert(
            "evidence".into(),
            json!({
                "kind": kind,
                "ref_id": t.trigger_evidence_ref_id.map(|u| u.to_string()).unwrap_or_default(),
            }),
        );
    }
    insert_uuid(&mut value, "rule_version_id", t.rule_version_id);
    insert_uuid(
        &mut value,
        "delegated_from_task_id",
        t.delegated_from_task_id,
    );
    insert_uuid(&mut value, "retry_of_task_id", t.retry_of_task_id);
    insert_uuid(&mut value, "rerun_of_task_id", t.rerun_of_task_id);
    Value::Object(value)
}

/// Go computeTaskKind — pure discriminator from FK shape.
fn compute_task_kind(t: &AgentTaskQueue) -> &'static str {
    if t.chat_session_id.is_some() {
        return "chat";
    }
    if t.autopilot_run_id.is_some() {
        return "autopilot";
    }
    if t.issue_id.is_none() {
        return "quick_create";
    }
    if t.trigger_comment_id.is_some() {
        return "comment";
    }
    "direct"
}

fn opt_time(t: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    t.map(crate::timefmt::rfc3339)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

fn insert_string(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        map.insert(key.into(), Value::String(value.into()));
    }
}

fn insert_uuid(map: &mut Map<String, Value>, key: &str, value: Option<uuid::Uuid>) {
    if let Some(value) = value {
        map.insert(key.into(), Value::String(value.to_string()));
    }
}

fn insert_string_array(map: &mut Map<String, Value>, key: &str, values: &[uuid::Uuid]) {
    if !values.is_empty() {
        map.insert(
            key.into(),
            Value::Array(
                values
                    .iter()
                    .map(|value| Value::String(value.to_string()))
                    .collect(),
            ),
        );
    }
}

fn insert_context_string(map: &mut Map<String, Value>, context: Option<&Value>, key: &str) {
    let Some(value) = context
        .and_then(Value::as_object)
        .and_then(|context| context.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    map.insert(key.into(), Value::String(value.into()));
}

/// Builds the full AgentTaskResponse map. Field order is irrelevant to JSON
/// consumers; key names mirror the Go tags exactly.
pub fn task_to_map(t: &AgentTaskQueue, workspace_id: &str) -> Value {
    let id = t.id.to_string();
    let mut value = Map::new();
    let is_message_bus_continuation = t
        .context
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|context| context.get("message_bus_parent_task_id"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());
    let public_status = if t.status == "deferred" && is_message_bus_continuation {
        "queued"
    } else {
        t.status.as_str()
    };
    let public_kind = if is_message_bus_continuation {
        "message_bus"
    } else {
        compute_task_kind(t)
    };
    value.insert("id".into(), Value::String(id.clone()));
    value.insert("agent_id".into(), Value::String(t.agent_id.to_string()));
    value.insert(
        "runtime_id".into(),
        Value::String(t.runtime_id.map(|id| id.to_string()).unwrap_or_default()),
    );
    value.insert(
        "issue_id".into(),
        Value::String(t.issue_id.map(|id| id.to_string()).unwrap_or_default()),
    );
    value.insert("workspace_id".into(), Value::String(workspace_id.into()));
    value.insert("status".into(), Value::String(public_status.into()));
    value.insert("priority".into(), json!(t.priority));
    value.insert("dispatched_at".into(), opt_time(t.dispatched_at));
    value.insert("started_at".into(), opt_time(t.started_at));
    value.insert("completed_at".into(), opt_time(t.completed_at));
    value.insert("result".into(), t.result.clone().unwrap_or(Value::Null));
    value.insert(
        "error".into(),
        t.error.clone().map(Value::String).unwrap_or(Value::Null),
    );
    value.insert("attempt".into(), json!(t.attempt));
    value.insert("max_attempts".into(), json!(t.max_attempts));
    value.insert(
        "created_at".into(),
        Value::String(crate::timefmt::rfc3339(t.created_at)),
    );
    value.insert(
        "delivered_comment_ids".into(),
        Value::Array(
            t.delivered_comment_ids
                .iter()
                .map(|id| Value::String(id.to_string()))
                .collect(),
        ),
    );
    value.insert("kind".into(), Value::String(public_kind.into()));
    value.insert("attribution".into(), attribution_base(t));

    insert_string(&mut value, "failure_reason", t.failure_reason.as_deref());
    insert_uuid(&mut value, "parent_task_id", t.parent_task_id);
    if t.is_leader_task {
        value.insert("is_leader_task".into(), Value::Bool(true));
    }
    insert_uuid(&mut value, "trigger_comment_id", t.trigger_comment_id);
    insert_string_array(
        &mut value,
        "coalesced_comment_ids",
        &t.coalesced_comment_ids,
    );
    if let Some(summary) = &t.trigger_summary {
        value.insert("trigger_summary".into(), Value::String(summary.clone()));
    }
    insert_string(&mut value, "handoff_note", t.handoff_note.as_deref());
    insert_string(&mut value, "work_dir", t.work_dir.as_deref());
    let relative = relative_work_dir(t.work_dir.as_deref().unwrap_or(""), workspace_id, &id);
    insert_string(&mut value, "relative_work_dir", Some(&relative));
    insert_string(
        &mut value,
        "durable_work_dir",
        t.durable_work_dir.as_deref(),
    );
    let relative_durable = relative_work_dir(t.durable_work_dir.as_deref().unwrap_or(""), "", "");
    insert_string(
        &mut value,
        "relative_durable_work_dir",
        Some(&relative_durable),
    );
    insert_uuid(&mut value, "chat_session_id", t.chat_session_id);
    insert_uuid(&mut value, "autopilot_run_id", t.autopilot_run_id);
    insert_string(&mut value, "branch_name", t.branch_name.as_deref());
    // These two IDs are safe routing metadata for the issue conversation UI.
    // Keep the rest of the internal task context server-private.
    insert_context_string(
        &mut value,
        t.context.as_ref(),
        "side_chat_parent_task_id",
    );
    insert_context_string(
        &mut value,
        t.context.as_ref(),
        "side_chat_root_comment_id",
    );

    Value::Object(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn task_fixture() -> AgentTaskQueue {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        AgentTaskQueue {
            id,
            agent_id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap(),
            accountable_user_id: None,
            attempt: 1,
            autopilot_run_id: None,
            branch_name: None,
            chat_finalize_deferred_at: None,
            chat_input_task_id: None,
            chat_session_id: None,
            coalesced_comment_ids: vec![],
            completed_at: None,
            context: None,
            created_at: Utc.with_ymd_and_hms(2026, 8, 23, 7, 0, 0).unwrap(),
            delegated_from_task_id: None,
            delivered_comment_ids: vec![],
            dispatched_at: None,
            durable_work_dir: None,
            error: None,
            escalation_for_task_id: None,
            failure_reason: None,
            fire_at: None,
            force_fresh_session: false,
            handoff_note: None,
            initiator_user_id: None,
            is_leader_task: false,
            issue_id: Some(Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap()),
            max_attempts: 3,
            originator_source: None,
            originator_user_id: None,
            parent_task_id: None,
            prepare_lease_expires_at: None,
            priority: 0,
            quick_actions_disabled: false,
            regenerate_quick_actions_for: None,
            rerun_of_task_id: None,
            result: None,
            retired_session_id: None,
            retry_of_task_id: None,
            rule_version_id: None,
            runtime_connected_apps: None,
            runtime_id: None,
            runtime_mcp_overlay: None,
            session_id: Some("provider-session-must-not-leak".into()),
            session_rollout_missing: false,
            squad_id: None,
            started_at: None,
            status: "queued".into(),
            trigger_comment_id: None,
            trigger_evidence_kind: None,
            trigger_evidence_ref_id: None,
            trigger_summary: None,
            wait_reason: Some("internal-wait-reason".into()),
            work_dir: None,
        }
    }

    #[test]
    fn user_task_wire_matches_go_omitempty_and_time_contract() {
        let value = task_to_map(&task_fixture(), "workspace-1");
        assert_eq!(value["created_at"], "2026-08-23T07:00:00Z");
        assert_eq!(value["delivered_comment_ids"], json!([]));
        assert_eq!(value["attribution"]["source"], "unattributed");
        assert_eq!(value["attribution"]["precise"], false);
        for absent in [
            "failure_reason",
            "parent_task_id",
            "is_leader_task",
            "trigger_comment_id",
            "coalesced_comment_ids",
            "trigger_summary",
            "handoff_note",
            "work_dir",
            "relative_work_dir",
            "session_id",
            "wait_reason",
            "fire_at",
        ] {
            assert!(value.get(absent).is_none(), "unexpected field {absent}");
        }
        assert!(value["attribution"].get("initiator").is_none());
        assert!(value["attribution"].get("originator").is_none());
        assert!(value["attribution"].get("evidence").is_none());
    }

    #[test]
    fn user_task_wire_preserves_optional_values_when_present() {
        let mut task = task_fixture();
        task.is_leader_task = true;
        task.failure_reason = Some("runtime_offline".into());
        task.accountable_user_id = Some(Uuid::nil());
        task.originator_source = Some("direct_human".into());
        task.dispatched_at = Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-23T07:00:00.987Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        let value = task_to_map(&task, "workspace-1");
        assert_eq!(value["is_leader_task"], true);
        assert_eq!(value["failure_reason"], "runtime_offline");
        assert_eq!(
            value["attribution"]["initiator"]["id"],
            Uuid::nil().to_string()
        );
        assert_eq!(value["dispatched_at"], "2026-08-23T07:00:00Z");
    }

    #[test]
    fn user_task_wire_exposes_only_side_chat_routing_context() {
        let mut task = task_fixture();
        task.context = Some(json!({
            "side_chat_parent_task_id": "main-task-1",
            "side_chat_root_comment_id": "comment-root-1",
            "internal_secret": "must-not-leak",
        }));

        let value = task_to_map(&task, "workspace-1");

        assert_eq!(value["side_chat_parent_task_id"], "main-task-1");
        assert_eq!(value["side_chat_root_comment_id"], "comment-root-1");
        assert!(value.get("internal_secret").is_none());
        assert!(value.get("context").is_none());
    }

    #[test]
    fn deferred_main_conversation_turn_is_publicly_queued() {
        let mut task = task_fixture();
        task.status = "deferred".into();
        task.context = Some(json!({
            "message_bus_parent_task_id": "main-task-1",
            "message_bus_messages": [{"content": "continue"}],
        }));

        let value = task_to_map(&task, "workspace-1");

        assert_eq!(value["status"], "queued");
        assert_eq!(value["kind"], "message_bus");
        assert!(value.get("message_bus_parent_task_id").is_none());
        assert!(value.get("message_bus_messages").is_none());
    }
}
