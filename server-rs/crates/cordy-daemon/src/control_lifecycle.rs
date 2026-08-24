//! Production consumption of daemon control-plane events.
//!
//! The WebSocket/HTTP transport in [`crate::manager`] owns connection safety;
//! this module connects its durable lifecycle signals to the daemon core's
//! registration recovery, runtime-profile refresh, heartbeat-action dispatch,
//! reconciliation, workspace sync, and task-claim orchestration boundaries.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use cordy_protocol::{DaemonHeartbeatAckPayload, RuntimeProfilesChangedPayload};

use crate::manager::{ControlEvent, DaemonControl};
use crate::reconcile::{ReconcileBroadcaster, WorkspaceChangeSignal};
use crate::repocache::Ctx;
use crate::wakeup::{signal_task_wakeup, TaskWakeup};

/// The daemon-core lifecycle surface reached by control events.
///
/// There are deliberately no default methods: production wiring must connect
/// every operation to the authoritative daemon state rather than silently
/// accepting an event through a compatibility no-op.
#[async_trait::async_trait]
pub(crate) trait DaemonControlLifecycle: Send + Sync + 'static {
    /// Go `handleRuntimeGone`: prune the stale runtime identity and run the
    /// coalesced register/recover-orphans flow under the workspace register
    /// lock.
    async fn handle_runtime_gone(&self, ctx: Ctx, runtime_id: String);

    /// Go `handleRuntimeProfilesChanged`: fetch and converge the workspace's
    /// custom runtime profiles through the normal registration boundary.
    async fn refresh_workspace_runtime_profiles(
        &self,
        ctx: Ctx,
        payload: RuntimeProfilesChangedPayload,
    );

    /// Go `handleHeartbeatActions`: fan out update/model/local-skill work from
    /// either an HTTP or WebSocket heartbeat response.
    async fn handle_heartbeat_actions(
        &self,
        ctx: Ctx,
        runtime_id: String,
        ack: DaemonHeartbeatAckPayload,
    );
}

/// Owns the single production receiver for [`ControlEvent`]. Heavy lifecycle
/// work runs in child tasks, matching the Go read pump's goroutine dispatch,
/// while coalesced wakeup/broadcast signals are applied synchronously.
pub(crate) struct ControlEventConsumer<H: DaemonControlLifecycle> {
    lifecycle: Arc<H>,
    task_wakeups: mpsc::Sender<TaskWakeup>,
    reconcile: Arc<ReconcileBroadcaster>,
    workspace_changes: Arc<WorkspaceChangeSignal>,
}

impl<H: DaemonControlLifecycle> ControlEventConsumer<H> {
    pub(crate) fn new(
        lifecycle: Arc<H>,
        task_wakeups: mpsc::Sender<TaskWakeup>,
        reconcile: Arc<ReconcileBroadcaster>,
        workspace_changes: Arc<WorkspaceChangeSignal>,
    ) -> Self {
        Self {
            lifecycle,
            task_wakeups,
            reconcile,
            workspace_changes,
        }
    }

    pub(crate) async fn run(&self, ctx: Ctx, mut events: mpsc::UnboundedReceiver<ControlEvent>) {
        let mut lifecycle_tasks = JoinSet::new();
        loop {
            tokio::select! {
                () = ctx.cancelled() => break,
                completed = lifecycle_tasks.join_next(), if !lifecycle_tasks.is_empty() => {
                    if let Some(Err(err)) = completed {
                        tracing::warn!(error = %err, "daemon control lifecycle task failed");
                    }
                }
                event = events.recv() => {
                    let Some(event) = event else { break };
                    self.route(&ctx, event, &mut lifecycle_tasks);
                }
            }
        }
        lifecycle_tasks.shutdown().await;
    }

    fn route(&self, ctx: &Ctx, event: ControlEvent, tasks: &mut JoinSet<()>) {
        match event {
            ControlEvent::Connected { runtime_ids } => {
                tracing::debug!(runtimes = runtime_ids.len(), "daemon control connected");
                signal_task_wakeup(&self.task_wakeups, "");
                self.reconcile.broadcast();
            }
            ControlEvent::TaskAvailable(payload) => {
                signal_task_wakeup(&self.task_wakeups, &payload.runtime_id);
            }
            ControlEvent::HeartbeatAck(ack) => {
                let lifecycle = Arc::clone(&self.lifecycle);
                let child = ctx.child();
                let runtime_id = ack.runtime_id.clone();
                tasks.spawn(async move {
                    lifecycle
                        .handle_heartbeat_actions(child, runtime_id, ack)
                        .await;
                });
            }
            ControlEvent::RuntimeGone { runtime_id } => {
                let lifecycle = Arc::clone(&self.lifecycle);
                let child = ctx.child();
                tasks.spawn(async move {
                    lifecycle.handle_runtime_gone(child, runtime_id).await;
                });
            }
            ControlEvent::RuntimeProfilesChanged(payload) => {
                let lifecycle = Arc::clone(&self.lifecycle);
                let child = ctx.child();
                tasks.spawn(async move {
                    lifecycle
                        .refresh_workspace_runtime_profiles(child, payload)
                        .await;
                });
            }
            ControlEvent::WorkspacesChanged => {
                self.workspace_changes.broadcast();
            }
        }
    }
}

/// Runs the control transport and its single lifecycle consumer under the same
/// daemon root context. Keeping both futures in one owner guarantees the event
/// receiver exists before the first WebSocket connect/heartbeat can publish.
pub(crate) async fn run_daemon_control<H: DaemonControlLifecycle>(
    ctx: Ctx,
    control: Arc<DaemonControl>,
    consumer: Arc<ControlEventConsumer<H>>,
    events: mpsc::UnboundedReceiver<ControlEvent>,
) {
    let transport_ctx = ctx.child();
    let consumer_ctx = ctx.child();
    tokio::join!(
        control.run(transport_ctx),
        consumer.run(consumer_ctx, events)
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::repocache::CancelCause;

    #[derive(Default)]
    struct RecordingLifecycle {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl DaemonControlLifecycle for RecordingLifecycle {
        async fn handle_runtime_gone(&self, _ctx: Ctx, runtime_id: String) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("runtime-gone:{runtime_id}"));
        }

        async fn refresh_workspace_runtime_profiles(
            &self,
            _ctx: Ctx,
            payload: RuntimeProfilesChangedPayload,
        ) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("profiles:{}", payload.workspace_id));
        }

        async fn handle_heartbeat_actions(
            &self,
            _ctx: Ctx,
            runtime_id: String,
            _ack: DaemonHeartbeatAckPayload,
        ) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("heartbeat:{runtime_id}"));
        }
    }

    #[tokio::test]
    async fn routes_every_control_event_to_its_core_boundary() {
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let (wakeup_tx, mut wakeup_rx) = mpsc::channel(4);
        let reconcile = Arc::new(ReconcileBroadcaster::new());
        let reconcile_snapshot = reconcile.notify();
        let workspace_changes = Arc::new(WorkspaceChangeSignal::new());
        let consumer = Arc::new(ControlEventConsumer::new(
            Arc::clone(&lifecycle),
            wakeup_tx,
            Arc::clone(&reconcile),
            Arc::clone(&workspace_changes),
        ));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let ctx = Ctx::new();
        let running = tokio::spawn({
            let consumer = Arc::clone(&consumer);
            let ctx = ctx.clone();
            async move { consumer.run(ctx, event_rx).await }
        });

        event_tx
            .send(ControlEvent::Connected {
                runtime_ids: vec!["r1".into()],
            })
            .unwrap();
        event_tx
            .send(ControlEvent::TaskAvailable(
                serde_json::from_value(json!({"runtime_id":"r1","task_id":"t1"})).unwrap(),
            ))
            .unwrap();
        event_tx
            .send(ControlEvent::RuntimeGone {
                runtime_id: "r1".into(),
            })
            .unwrap();
        event_tx
            .send(ControlEvent::RuntimeProfilesChanged(
                serde_json::from_value(json!({"workspace_id":"w1","runtime_profile_id":"p1"}))
                    .unwrap(),
            ))
            .unwrap();
        event_tx
            .send(ControlEvent::HeartbeatAck(
                serde_json::from_value(json!({"runtime_id":"r1","status":"ok"})).unwrap(),
            ))
            .unwrap();
        event_tx.send(ControlEvent::WorkspacesChanged).unwrap();

        assert_eq!(wakeup_rx.recv().await.unwrap().runtime_id, "");
        assert_eq!(wakeup_rx.recv().await.unwrap().runtime_id, "r1");
        assert!(reconcile_snapshot.is_closed());
        workspace_changes.recv().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if lifecycle.calls.lock().unwrap().len() == 3 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let mut calls = lifecycle.calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls,
            vec!["heartbeat:r1", "profiles:w1", "runtime-gone:r1"]
        );
        ctx.cancel_with(CancelCause::Cancelled);
        running.await.unwrap();
    }
}
