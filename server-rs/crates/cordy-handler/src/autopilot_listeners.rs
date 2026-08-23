//! Autopilot run event listeners — port of
//! `server/cmd/server/autopilot_listeners.go`.

use std::sync::Arc;

use cordy_events::{Bus, Event};
use cordy_service::autopilot::AutopilotService;
use uuid::Uuid;

/// Keeps Autopilot runs synchronized with the issue or task created for them.
///
/// The Rust database layer is async while the in-process bus is deliberately
/// synchronous. Each listener therefore performs only payload classification
/// inline and hands the database work to Tokio. The terminal row is committed
/// before these events are published, so the detached load always observes the
/// state that caused the event.
pub(crate) fn register(bus: &Bus, service: Arc<AutopilotService>) {
    let issue_service = service.clone();
    bus.subscribe(cordy_protocol::EVENT_ISSUE_UPDATED, move |event| {
        let Some(issue_id) = terminal_issue_id(event) else {
            return;
        };
        let service = issue_service.clone();
        tokio::spawn(async move {
            match cordy_db::queries::issue::get_issue(&service.pool, issue_id).await {
                Ok(Some(issue)) => service.sync_run_from_issue(&issue).await,
                Ok(None) => {}
                Err(error) => tracing::debug!(
                    %issue_id,
                    %error,
                    "autopilot listener: failed to load issue"
                ),
            }
        });
    });

    subscribe_task_event(
        bus,
        service.clone(),
        cordy_protocol::EVENT_TASK_COMPLETED,
        false,
    );
    subscribe_task_event(
        bus,
        service.clone(),
        cordy_protocol::EVENT_TASK_FAILED,
        true,
    );
    subscribe_task_event(bus, service, cordy_protocol::EVENT_TASK_CANCELLED, false);
}

fn subscribe_task_event(
    bus: &Bus,
    service: Arc<AutopilotService>,
    event_type: &'static str,
    sync_linked_issue_failure: bool,
) {
    bus.subscribe(event_type, move |event| {
        let Some(task_id) = task_id(event) else {
            return;
        };
        let service = service.clone();
        tokio::spawn(async move {
            let Ok(Some(task)) =
                cordy_db::queries::agent::get_agent_task(&service.pool, task_id).await
            else {
                return;
            };
            if task.autopilot_run_id.is_some() {
                service.sync_run_from_task(&task).await;
            } else if sync_linked_issue_failure {
                service.sync_run_from_linked_issue_task(&task).await;
            }
        });
    });
}

/// Applies the same cheap built-in-status gate as Go. Custom keys must pass
/// through because their terminal behavior requires a catalog lookup in
/// `sync_run_from_issue`.
fn terminal_issue_id(event: &Event) -> Option<Uuid> {
    if event
        .payload
        .get("status_changed")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return None;
    }
    let issue = event.payload.get("issue")?;
    let status = issue.get("status")?.as_str()?;
    if cordy_service::issue_status::is_built_in(status)
        && !matches!(status, "done" | "in_review" | "cancelled" | "blocked")
    {
        return None;
    }
    issue.get("id")?.as_str()?.parse().ok()
}

fn task_id(event: &Event) -> Option<Uuid> {
    event.payload.get("task_id")?.as_str()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ISSUE_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TASK_ID: &str = "22222222-2222-4222-8222-222222222222";

    fn issue_event(status: &str, changed: bool) -> Event {
        Event {
            event_type: cordy_protocol::EVENT_ISSUE_UPDATED.into(),
            payload: json!({
                "status_changed": changed,
                "issue": {"id": ISSUE_ID, "status": status},
            }),
            ..Default::default()
        }
    }

    #[test]
    fn terminal_builtin_statuses_are_selected() {
        for status in ["done", "in_review", "cancelled", "blocked"] {
            assert_eq!(
                terminal_issue_id(&issue_event(status, true)),
                ISSUE_ID.parse().ok()
            );
        }
    }

    #[test]
    fn routine_builtin_statuses_and_unchanged_updates_are_skipped() {
        for status in ["backlog", "todo", "in_progress"] {
            assert_eq!(terminal_issue_id(&issue_event(status, true)), None);
        }
        assert_eq!(terminal_issue_id(&issue_event("done", false)), None);
    }

    #[test]
    fn custom_statuses_pass_through_for_catalog_resolution() {
        assert_eq!(
            terminal_issue_id(&issue_event("human_review", true)),
            ISSUE_ID.parse().ok()
        );
    }

    #[test]
    fn malformed_issue_payloads_fail_closed() {
        for payload in [
            json!(null),
            json!({"status_changed": true}),
            json!({"status_changed": true, "issue": {"id": "bad", "status": "done"}}),
            json!({"status_changed": "true", "issue": {"id": ISSUE_ID, "status": "done"}}),
        ] {
            assert_eq!(
                terminal_issue_id(&Event {
                    payload,
                    ..Default::default()
                }),
                None
            );
        }
    }

    #[test]
    fn task_id_comes_only_from_a_valid_payload_field() {
        assert_eq!(
            task_id(&Event {
                task_id: "33333333-3333-4333-8333-333333333333".into(),
                payload: json!({"task_id": TASK_ID}),
                ..Default::default()
            }),
            TASK_ID.parse().ok()
        );
        assert_eq!(task_id(&Event::default()), None);
        assert_eq!(
            task_id(&Event {
                payload: json!({"task_id": "bad"}),
                ..Default::default()
            }),
            None
        );
    }
}
