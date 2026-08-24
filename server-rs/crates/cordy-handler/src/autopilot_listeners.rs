//! Autopilot run event listeners — port of
//! `server/cmd/server/autopilot_listeners.go`.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordy_events::{Bus, Event};
use cordy_service::autopilot::AutopilotService;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Keeps Autopilot runs synchronized with the issue or task created for them.
///
/// The Rust database layer is async while the in-process bus is deliberately
/// synchronous. Classification remains inline, while every admitted database
/// task is tracked for bounded production shutdown.
pub struct AutopilotEventListeners {
    bus: Arc<Bus>,
    service: Arc<AutopilotService>,
    cancel: CancellationToken,
    started: AtomicBool,
    accepting_tasks: AtomicBool,
    tasks: Mutex<JoinSet<()>>,
}

impl AutopilotEventListeners {
    pub fn new(bus: Arc<Bus>, service: Arc<AutopilotService>) -> Arc<Self> {
        Arc::new(Self {
            bus,
            service,
            cancel: CancellationToken::new(),
            started: AtomicBool::new(false),
            accepting_tasks: AtomicBool::new(false),
            tasks: Mutex::new(JoinSet::new()),
        })
    }

    pub fn start(
        self: &Arc<Self>,
        parent: CancellationToken,
    ) -> Option<AutopilotEventListenersRuntime> {
        if self.started.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.accepting_tasks.store(true, Ordering::Release);
        let issue_listeners = self.clone();
        self.bus
            .subscribe(cordy_protocol::EVENT_ISSUE_UPDATED, move |event| {
                issue_listeners.dispatch_issue(event);
            });

        self.subscribe_task_event(cordy_protocol::EVENT_TASK_COMPLETED, false);
        self.subscribe_task_event(cordy_protocol::EVENT_TASK_FAILED, true);
        self.subscribe_task_event(cordy_protocol::EVENT_TASK_CANCELLED, false);

        let listeners = self.clone();
        self.spawn_task(async move {
            tokio::select! {
                _ = parent.cancelled() => {}
                _ = listeners.cancel.cancelled() => {}
            }
            listeners.cancel.cancel();
        });
        Some(AutopilotEventListenersRuntime {
            listeners: self.clone(),
        })
    }

    fn dispatch_issue(&self, event: &Event) {
        let Some(issue_id) = terminal_issue_id(event) else {
            return;
        };
        let service = self.service.clone();
        self.spawn_task(async move {
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
    }

    fn subscribe_task_event(self: &Arc<Self>, event_type: &'static str, linked_failure: bool) {
        let listeners = self.clone();
        self.bus.subscribe(event_type, move |event| {
            listeners.dispatch_task(event, linked_failure);
        });
    }

    fn dispatch_task(&self, event: &Event, sync_linked_issue_failure: bool) {
        let Some(task_id) = task_id(event) else {
            return;
        };
        let service = self.service.clone();
        self.spawn_task(async move {
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
    }

    fn spawn_task(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::error!(%error, "autopilot event listener task panicked");
            }
        }
        if self.accepting_tasks.load(Ordering::Acquire) {
            tasks.spawn(task);
        }
    }

    fn stop_accepting_tasks(&self) -> JoinSet<()> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.accepting_tasks.store(false, Ordering::Release);
        std::mem::take(&mut *tasks)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutopilotEventShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

pub struct AutopilotEventListenersRuntime {
    listeners: Arc<AutopilotEventListeners>,
}

impl AutopilotEventListenersRuntime {
    pub async fn shutdown(self, timeout: Duration) -> AutopilotEventShutdownOutcome {
        self.listeners.cancel.cancel();
        let mut tasks = self.listeners.stop_accepting_tasks();
        let mut panicked = false;
        let joined = tokio::time::timeout(timeout, async {
            while let Some(result) = tasks.join_next().await {
                if result.is_err() {
                    panicked = true;
                }
            }
        })
        .await;
        if joined.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            return AutopilotEventShutdownOutcome::TimedOut;
        }
        if panicked {
            AutopilotEventShutdownOutcome::Panicked
        } else {
            AutopilotEventShutdownOutcome::Stopped
        }
    }
}

impl Drop for AutopilotEventListenersRuntime {
    fn drop(&mut self) {
        self.listeners.cancel.cancel();
        let mut tasks = self.listeners.stop_accepting_tasks();
        tasks.abort_all();
    }
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
