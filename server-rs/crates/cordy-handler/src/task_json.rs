//! Task wire-shape builder — port of Go `taskToResponse`
//! (server/internal/handler/agent.go:693) as a JSON map. Key names and
//! null-vs-absent behavior match the Go struct tags byte-for-byte so clients
//! type both shapes identically.

use serde_json::{json, Value};

use cordy_db::models::AgentTaskQueue;

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
    let mut evidence = Value::Null;
    if let Some(kind) = t.trigger_evidence_kind.as_deref().filter(|k| !k.is_empty()) {
        evidence = json!({
            "kind": kind,
            "ref_id": t.trigger_evidence_ref_id.map(|u| u.to_string()).unwrap_or_default(),
        });
    }
    json!({
        "source": if source.is_empty() { "unattributed".to_string() } else { source },
        "precise": precise,
        "initiator": t.accountable_user_id.map(|u| json!({ "id": u.to_string() })),
        "originator": t.originator_user_id.map(|u| json!({ "id": u.to_string() })),
        "evidence": evidence,
        "rule_version_id": t.rule_version_id.map(|u| u.to_string()).unwrap_or_default(),
        "delegated_from_task_id": t.delegated_from_task_id.map(|u| u.to_string()).unwrap_or_default(),
        "retry_of_task_id": t.retry_of_task_id.map(|u| u.to_string()).unwrap_or_default(),
        "rerun_of_task_id": t.rerun_of_task_id.map(|u| u.to_string()).unwrap_or_default(),
    })
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
    t.map(crate::timefmt::rfc3339_nano)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

/// Builds the full AgentTaskResponse map. Field order is irrelevant to JSON
/// consumers; key names mirror the Go tags exactly.
pub fn task_to_map(t: &AgentTaskQueue, workspace_id: &str) -> Value {
    let id = t.id.to_string();
    let result = t.result.clone().unwrap_or(Value::Null);
    json!({
        "id": id,
        "agent_id": t.agent_id.to_string(),
        "runtime_id": t.runtime_id.map(|u| u.to_string()).unwrap_or_default(),
        "issue_id": t.issue_id.map(|u| u.to_string()).unwrap_or_default(),
        "workspace_id": workspace_id,
        "status": t.status,
        "priority": t.priority,
        "dispatched_at": opt_time(t.dispatched_at),
        "started_at": opt_time(t.started_at),
        "completed_at": opt_time(t.completed_at),
        "result": result,
        "error": t.error.clone(),
        "failure_reason": t.failure_reason.clone().unwrap_or_default(),
        "attempt": t.attempt,
        "max_attempts": t.max_attempts,
        "parent_task_id": t.parent_task_id.map(|u| u.to_string()),
        "is_leader_task": if t.is_leader_task { Some(true) } else { None },
        "created_at": crate::timefmt::rfc3339(t.created_at),
        "trigger_comment_id": t.trigger_comment_id.map(|u| u.to_string()),
        "coalesced_comment_ids": t.coalesced_comment_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "delivered_comment_ids": t.delivered_comment_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "trigger_summary": t.trigger_summary.clone(),
        "handoff_note": t.handoff_note.clone().unwrap_or_default(),
        "work_dir": t.work_dir.clone().unwrap_or_default(),
        "relative_work_dir": relative_work_dir(
            t.work_dir.as_deref().unwrap_or(""),
            workspace_id,
            &id,
        ),
        "durable_work_dir": t.durable_work_dir.clone().unwrap_or_default(),
        "relative_durable_work_dir": relative_work_dir(
            t.durable_work_dir.as_deref().unwrap_or(""),
            "",
            "",
        ),
        "chat_session_id": t.chat_session_id.map(|u| u.to_string()).unwrap_or_default(),
        "autopilot_run_id": t.autopilot_run_id.map(|u| u.to_string()).unwrap_or_default(),
        "kind": compute_task_kind(t),
        "attribution": attribution_base(t),
        "session_id": t.session_id.clone(),
        "squad_id": t.squad_id.map(|u| u.to_string()).unwrap_or_default(),
        "branch_name": t.branch_name.clone().unwrap_or_default(),
        "wait_reason": t.wait_reason.clone(),
        "fire_at": opt_time(t.fire_at),
    })
}
