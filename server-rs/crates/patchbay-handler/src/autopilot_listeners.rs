//! Autopilot run event listeners.
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use patchbay_events::{Bus, Event};
use patchbay_service::autopilot::AutopilotService;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
enum AutopilotEventWork {
    Issue(Uuid),
    Task {
        task_id: Uuid,
        sync_linked_issue_failure: bool,
    },
}

type WorkFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type WorkProcessor = Arc<dyn Fn(AutopilotEventWork) -> WorkFuture + Send + Sync>;

/// Keeps Autopilot runs synchronized with the issue or task created for them.
///
/// The Rust database layer is async while the in-process bus is deliberately
/// synchronous. Classification remains inline, while one owned FIFO consumer
/// preserves publication order across the admitted database operations.
pub struct AutopilotEventListeners {
    bus: Arc<Bus>,
    processor: WorkProcessor,
    started: AtomicBool,
    accepting_tasks: AtomicBool,
    sender: Mutex<Option<mpsc::UnboundedSender<AutopilotEventWork>>>,
}

impl AutopilotEventListeners {
    pub fn new(bus: Arc<Bus>, service: Arc<AutopilotService>) -> Arc<Self> {
        let processor: WorkProcessor = Arc::new(move |work| {
            let service = service.clone();
            Box::pin(async move { handle_work(&service, work).await })
        });
        Self::with_processor(bus, processor)
    }

    fn with_processor(bus: Arc<Bus>, processor: WorkProcessor) -> Arc<Self> {
        Arc::new(Self {
            bus,
            processor,
            started: AtomicBool::new(false),
            accepting_tasks: AtomicBool::new(false),
            sender: Mutex::new(None),
        })
    }

    pub fn start(
        self: &Arc<Self>,
        parent: CancellationToken,
    ) -> Option<AutopilotEventListenersRuntime> {
        if self.started.swap(true, Ordering::AcqRel) {
            return None;
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        *self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        self.accepting_tasks.store(true, Ordering::Release);
        let issue_listeners = self.clone();
        self.bus
            .subscribe(patchbay_protocol::EVENT_ISSUE_UPDATED, move |event| {
                issue_listeners.dispatch_issue(event);
            });

        self.subscribe_task_event(patchbay_protocol::EVENT_TASK_COMPLETED, false);
        self.subscribe_task_event(patchbay_protocol::EVENT_TASK_FAILED, true);
        self.subscribe_task_event(patchbay_protocol::EVENT_TASK_CANCELLED, false);

        let listeners = self.clone();
        let processor = self.processor.clone();
        let task = tokio::spawn(async move {
            AutopilotEventWorker {
                receiver,
                processor,
            }
            .run(parent, listeners)
            .await;
        });
        Some(AutopilotEventListenersRuntime {
            listeners: self.clone(),
            task: Some(task),
        })
    }

    fn dispatch_issue(&self, event: &Event) {
        let Some(issue_id) = terminal_issue_id(event) else {
            return;
        };
        self.enqueue(AutopilotEventWork::Issue(issue_id));
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
        self.enqueue(AutopilotEventWork::Task {
            task_id,
            sync_linked_issue_failure,
        });
    }

    fn enqueue(&self, work: AutopilotEventWork) {
        if !self.accepting_tasks.load(Ordering::Acquire) {
            return;
        }
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(work);
        }
    }

    fn stop_accepting_tasks(&self) {
        self.accepting_tasks.store(false, Ordering::Release);
        self.sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

async fn handle_work(service: &Arc<AutopilotService>, work: AutopilotEventWork) {
    match work {
        AutopilotEventWork::Issue(issue_id) => {
            match patchbay_db::queries::issue::get_issue(&service.pool, issue_id).await {
                Ok(Some(issue)) => service.sync_run_from_issue(&issue).await,
                Ok(None) => {}
                Err(error) => tracing::debug!(
                    %issue_id,
                    %error,
                    "autopilot listener: failed to load issue"
                ),
            }
        }
        AutopilotEventWork::Task {
            task_id,
            sync_linked_issue_failure,
        } => {
            let Ok(Some(task)) =
                patchbay_db::queries::agent::get_agent_task(&service.pool, task_id).await
            else {
                return;
            };
            if task.autopilot_run_id.is_some() {
                service.sync_run_from_task(&task).await;
            } else if sync_linked_issue_failure {
                service.sync_run_from_linked_issue_task(&task).await;
            }
        }
    }
}

pub(crate) async fn handle_event(service: &Arc<AutopilotService>, event: &Event) {
    let work = match event.event_type.as_str() {
        patchbay_protocol::EVENT_ISSUE_UPDATED => {
            terminal_issue_id(event).map(AutopilotEventWork::Issue)
        }
        patchbay_protocol::EVENT_TASK_COMPLETED | patchbay_protocol::EVENT_TASK_CANCELLED => {
            task_id(event).map(|task_id| AutopilotEventWork::Task {
                task_id,
                sync_linked_issue_failure: false,
            })
        }
        patchbay_protocol::EVENT_TASK_FAILED => {
            task_id(event).map(|task_id| AutopilotEventWork::Task {
                task_id,
                sync_linked_issue_failure: true,
            })
        }
        _ => None,
    };
    if let Some(work) = work {
        handle_work(service, work).await;
    }
}

struct AutopilotEventWorker {
    receiver: mpsc::UnboundedReceiver<AutopilotEventWork>,
    processor: WorkProcessor,
}

impl AutopilotEventWorker {
    async fn run(mut self, parent: CancellationToken, listeners: Arc<AutopilotEventListeners>) {
        loop {
            tokio::select! {
                biased;
                _ = parent.cancelled() => {
                    listeners.stop_accepting_tasks();
                    self.receiver.close();
                    while self.process_next().await {}
                    return;
                }
                processed = self.process_next() => {
                    if !processed {
                        return;
                    }
                }
            }
        }
    }

    async fn process_next(&mut self) -> bool {
        let Some(work) = self.receiver.recv().await else {
            return false;
        };
        if let Err(panic) = AssertUnwindSafe((self.processor)(work))
            .catch_unwind()
            .await
        {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(recovered = %detail, "autopilot event listener task panicked");
        }
        true
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
    task: Option<JoinHandle<()>>,
}

impl AutopilotEventListenersRuntime {
    pub async fn shutdown(mut self, timeout: Duration) -> AutopilotEventShutdownOutcome {
        self.listeners.stop_accepting_tasks();
        let Some(mut task) = self.task.take() else {
            return AutopilotEventShutdownOutcome::Stopped;
        };
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => AutopilotEventShutdownOutcome::Stopped,
            Ok(Err(_)) => AutopilotEventShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                AutopilotEventShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for AutopilotEventListenersRuntime {
    fn drop(&mut self) {
        self.listeners.stop_accepting_tasks();
        if let Some(task) = self.task.take() {
            task.abort();
        }
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
    if patchbay_service::issue_status::is_built_in(status)
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
            event_type: patchbay_protocol::EVENT_ISSUE_UPDATED.into(),
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

    #[tokio::test]
    async fn worker_processes_admitted_events_in_fifo_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let processor: WorkProcessor = Arc::new({
            let log = log.clone();
            move |work| {
                let log = log.clone();
                Box::pin(async move {
                    let kind = match work {
                        AutopilotEventWork::Issue(_) => "issue",
                        AutopilotEventWork::Task { .. } => "task",
                    };
                    log.lock().unwrap().push(kind);
                })
            }
        });
        let (sender, receiver) = mpsc::unbounded_channel();
        sender
            .send(AutopilotEventWork::Issue(ISSUE_ID.parse().unwrap()))
            .unwrap();
        sender
            .send(AutopilotEventWork::Task {
                task_id: TASK_ID.parse().unwrap(),
                sync_linked_issue_failure: true,
            })
            .unwrap();
        drop(sender);
        let mut worker = AutopilotEventWorker {
            receiver,
            processor,
        };

        assert!(worker.process_next().await);
        assert!(worker.process_next().await);
        assert!(!worker.process_next().await);
        assert_eq!(*log.lock().unwrap(), vec!["issue", "task"]);
    }
}
