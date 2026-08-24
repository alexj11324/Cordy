//! Ordered subscriber, activity, and notification event side effects.
//!
//! Go registers these listeners in that order and executes each bus callback
//! synchronously. SQLx requires async work, so this runtime moves one whole
//! event into a tracked task while preserving the ordering inside that task.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cordy_events::{Bus, Event};
use sqlx::PgPool;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

const ORDERED_EVENT_TYPES: [&str; 7] = [
    cordy_protocol::EVENT_ISSUE_CREATED,
    cordy_protocol::EVENT_ISSUE_UPDATED,
    cordy_protocol::EVENT_COMMENT_CREATED,
    cordy_protocol::EVENT_TASK_COMPLETED,
    cordy_protocol::EVENT_TASK_FAILED,
    cordy_protocol::EVENT_ISSUE_REACTION_ADDED,
    cordy_protocol::EVENT_REACTION_ADDED,
];

pub struct OrderedEventSideEffects {
    pool: PgPool,
    bus: Arc<Bus>,
    cancel: CancellationToken,
    started: AtomicBool,
    accepting_tasks: AtomicBool,
    tasks: Mutex<JoinSet<()>>,
}

impl OrderedEventSideEffects {
    pub fn new(pool: PgPool, bus: Arc<Bus>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            bus,
            cancel: CancellationToken::new(),
            started: AtomicBool::new(false),
            accepting_tasks: AtomicBool::new(false),
            tasks: Mutex::new(JoinSet::new()),
        })
    }

    pub fn start(
        self: &Arc<Self>,
        parent: CancellationToken,
    ) -> Option<OrderedEventSideEffectsRuntime> {
        if self.started.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.accepting_tasks.store(true, Ordering::Release);
        for event_type in ORDERED_EVENT_TYPES {
            let side_effects = self.clone();
            self.bus.subscribe(event_type, move |event| {
                side_effects.dispatch(event.clone());
            });
        }
        let side_effects = self.clone();
        self.spawn_task(async move {
            tokio::select! {
                _ = parent.cancelled() => {}
                _ = side_effects.cancel.cancelled() => {}
            }
            side_effects.cancel.cancel();
        });
        Some(OrderedEventSideEffectsRuntime {
            side_effects: self.clone(),
        })
    }

    fn dispatch(self: &Arc<Self>, event: Event) {
        let side_effects = self.clone();
        self.spawn_task(async move {
            crate::subscriber_activity_listeners::handle_event(
                &side_effects.pool,
                &side_effects.bus,
                &event,
            )
            .await;
            crate::notification_listeners::handle_event(
                side_effects.pool.clone(),
                side_effects.bus.clone(),
                event,
            )
            .await;
        });
    }

    fn spawn_task(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::error!(%error, "ordered event side-effect task panicked");
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
pub enum OrderedEventShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

pub struct OrderedEventSideEffectsRuntime {
    side_effects: Arc<OrderedEventSideEffects>,
}

impl OrderedEventSideEffectsRuntime {
    pub async fn shutdown(self, timeout: Duration) -> OrderedEventShutdownOutcome {
        self.side_effects.cancel.cancel();
        let mut tasks = self.side_effects.stop_accepting_tasks();
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
            return OrderedEventShutdownOutcome::TimedOut;
        }
        if panicked {
            OrderedEventShutdownOutcome::Panicked
        } else {
            OrderedEventShutdownOutcome::Stopped
        }
    }
}

impl Drop for OrderedEventSideEffectsRuntime {
    fn drop(&mut self) {
        self.side_effects.cancel.cancel();
        let mut tasks = self.side_effects.stop_accepting_tasks();
        tasks.abort_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runtime_start_is_idempotent_and_shutdown_is_owned() {
        let pool = PgPool::connect_lazy("postgres://invalid/invalid").expect("valid test URL");
        let side_effects = OrderedEventSideEffects::new(pool, Arc::new(Bus::new()));
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
}
