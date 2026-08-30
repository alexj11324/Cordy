//! Ordered subscriber, activity, and notification event side effects.
//!
//! Go registers these listeners in that order and executes each bus callback
//! synchronously. SQLx requires async work, so this runtime admits events into
//! one owned FIFO consumer. That preserves both the listener order within an
//! event and the publication order across consecutive events.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use patchbay_events::{Bus, Event};
use patchbay_service::autopilot::AutopilotService;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

type EventFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type EventProcessor = Arc<dyn Fn(Event) -> EventFuture + Send + Sync>;

const ORDERED_EVENT_TYPES: [&str; 8] = [
    patchbay_protocol::EVENT_ISSUE_CREATED,
    patchbay_protocol::EVENT_ISSUE_UPDATED,
    patchbay_protocol::EVENT_COMMENT_CREATED,
    patchbay_protocol::EVENT_TASK_COMPLETED,
    patchbay_protocol::EVENT_TASK_FAILED,
    patchbay_protocol::EVENT_TASK_CANCELLED,
    patchbay_protocol::EVENT_ISSUE_REACTION_ADDED,
    patchbay_protocol::EVENT_REACTION_ADDED,
];

pub struct OrderedEventSideEffects {
    bus: Arc<Bus>,
    processor: EventProcessor,
    started: AtomicBool,
    accepting_tasks: AtomicBool,
    sender: Mutex<Option<mpsc::UnboundedSender<Event>>>,
}

impl OrderedEventSideEffects {
    pub fn new(pool: PgPool, bus: Arc<Bus>, autopilots: Arc<AutopilotService>) -> Arc<Self> {
        let processor_bus = bus.clone();
        let processor: EventProcessor = Arc::new(move |event| {
            let pool = pool.clone();
            let bus = processor_bus.clone();
            let autopilots = autopilots.clone();
            Box::pin(async move {
                let coordinator_publication = Self::is_coordination_publication(&event);
                if let Err(error) =
                    crate::subscriber_activity_listeners::handle_event(&pool, &bus, &event).await
                {
                    tracing::error!(%error, "ordered subscriber/activity side effects failed");
                    if coordinator_publication {
                        return;
                    }
                }
                if let Err(error) =
                    crate::notification_listeners::handle_event(pool.clone(), bus, event.clone())
                        .await
                {
                    tracing::error!(%error, "ordered notification side effects failed");
                    if coordinator_publication {
                        return;
                    }
                }
                if let Err(error) =
                    crate::autopilot_listeners::handle_event(&autopilots, &event).await
                {
                    tracing::error!(%error, "ordered Autopilot side effects failed");
                    if coordinator_publication {
                        return;
                    }
                }
                if let Err(error) =
                    patchbay_service::coordination::acknowledge_coordination_publication(
                        &pool, &event,
                    )
                    .await
                {
                    tracing::error!(%error, "ordered event side-effect acknowledgement failed");
                }
            })
        });
        Self::with_processor(bus, processor)
    }

    fn is_coordination_publication(event: &Event) -> bool {
        event
            .payload
            .get("coordination_event_id")
            .and_then(Value::as_str)
            .is_some()
            && matches!(
                event
                    .payload
                    .get("coordination_publication")
                    .and_then(Value::as_str),
                Some("review_handoff") | Some("reviewer_replacement") | Some("assignment_activity")
            )
    }

    fn with_processor(bus: Arc<Bus>, processor: EventProcessor) -> Arc<Self> {
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
    ) -> Option<OrderedEventSideEffectsRuntime> {
        if self.started.swap(true, Ordering::AcqRel) {
            return None;
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        *self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        self.accepting_tasks.store(true, Ordering::Release);
        for event_type in ORDERED_EVENT_TYPES {
            let side_effects = self.clone();
            self.bus.subscribe(event_type, move |event| {
                side_effects.dispatch(event.clone());
            });
        }
        let side_effects = self.clone();
        let processor = self.processor.clone();
        let task = tokio::spawn(async move {
            OrderedEventWorker {
                receiver,
                processor,
            }
            .run(parent, side_effects)
            .await;
        });
        Some(OrderedEventSideEffectsRuntime {
            side_effects: self.clone(),
            task: Some(task),
        })
    }

    fn dispatch(&self, event: Event) {
        if !self.accepting_tasks.load(Ordering::Acquire) {
            return;
        }
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(event);
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

struct OrderedEventWorker {
    receiver: mpsc::UnboundedReceiver<Event>,
    processor: EventProcessor,
}

impl OrderedEventWorker {
    async fn run(mut self, parent: CancellationToken, side_effects: Arc<OrderedEventSideEffects>) {
        loop {
            tokio::select! {
                biased;
                _ = parent.cancelled() => {
                    side_effects.stop_accepting_tasks();
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
        let Some(event) = self.receiver.recv().await else {
            return false;
        };
        if let Err(panic) = AssertUnwindSafe((self.processor)(event))
            .catch_unwind()
            .await
        {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::error!(recovered = %detail, "ordered event side-effect task panicked");
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderedEventShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

pub struct OrderedEventSideEffectsRuntime {
    side_effects: Arc<OrderedEventSideEffects>,
    task: Option<JoinHandle<()>>,
}

impl OrderedEventSideEffectsRuntime {
    pub async fn shutdown(mut self, timeout: Duration) -> OrderedEventShutdownOutcome {
        self.side_effects.stop_accepting_tasks();
        let Some(mut task) = self.task.take() else {
            return OrderedEventShutdownOutcome::Stopped;
        };
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => OrderedEventShutdownOutcome::Stopped,
            Ok(Err(_)) => OrderedEventShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                OrderedEventShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for OrderedEventSideEffectsRuntime {
    fn drop(&mut self) {
        self.side_effects.stop_accepting_tasks();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn runtime_start_is_idempotent_and_shutdown_is_owned() {
        let side_effects = OrderedEventSideEffects::with_processor(
            Arc::new(Bus::new()),
            Arc::new(|_| Box::pin(async {})),
        );
        let root = CancellationToken::new();
        let runtime = side_effects
            .start(root.child_token())
            .expect("first start owns runtime");
        assert!(side_effects.start(root.child_token()).is_none());

        root.cancel();
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)).await,
            OrderedEventShutdownOutcome::Stopped
        );
    }

    #[tokio::test]
    async fn consecutive_events_are_processed_by_one_fifo_consumer() {
        let bus = Arc::new(Bus::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let first_started = Arc::new(tokio::sync::Notify::new());
        let release_first = Arc::new(tokio::sync::Notify::new());
        let processor: EventProcessor = Arc::new({
            let log = log.clone();
            let first_started = first_started.clone();
            let release_first = release_first.clone();
            move |event| {
                let log = log.clone();
                let first_started = first_started.clone();
                let release_first = release_first.clone();
                Box::pin(async move {
                    let sequence = event.payload["sequence"].as_u64().unwrap();
                    log.lock().unwrap().push((sequence, "start"));
                    if sequence == 1 {
                        first_started.notify_one();
                        release_first.notified().await;
                    }
                    log.lock().unwrap().push((sequence, "finish"));
                })
            }
        });
        let side_effects = OrderedEventSideEffects::with_processor(bus.clone(), processor);
        let runtime = side_effects
            .start(CancellationToken::new())
            .expect("runtime starts");

        bus.publish(&Event {
            event_type: patchbay_protocol::EVENT_ISSUE_CREATED.into(),
            payload: json!({"sequence": 1}),
            ..Default::default()
        });
        first_started.notified().await;
        bus.publish(&Event {
            event_type: patchbay_protocol::EVENT_ISSUE_CREATED.into(),
            payload: json!({"sequence": 2}),
            ..Default::default()
        });
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        assert_eq!(*log.lock().unwrap(), vec![(1, "start")]);

        release_first.notify_one();
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)).await,
            OrderedEventShutdownOutcome::Stopped
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec![(1, "start"), (1, "finish"), (2, "start"), (2, "finish")]
        );
    }

    #[test]
    fn only_durable_coordination_publications_use_replay_on_error() {
        let coordinator_event = Event {
            payload: json!({
                "coordination_event_id": "11111111-1111-4111-8111-111111111111",
                "coordination_publication": "review_handoff",
            }),
            ..Default::default()
        };
        let ordinary_event = Event {
            payload: json!({"coordination_publication": "review_handoff"}),
            ..Default::default()
        };
        assert!(OrderedEventSideEffects::is_coordination_publication(
            &coordinator_event
        ));
        assert!(!OrderedEventSideEffects::is_coordination_publication(
            &ordinary_event
        ));
    }
}
