//! The daemon wakeup
//! publisher that bridges process events to the local hub and, when Redis is
//! configured, to the shared realtime relay.
//!
//! Symbol map (Go → Rust):
//! - `RelayNotifier` / `NewRelayNotifier` → [`RelayNotifier`] /
//!   [`RelayNotifier::new`]
//! - `NotifyTaskAvailable` / `NotifyRuntimeProfilesChanged` /
//!   `NotifyWorkspacesChanged` / `NotifyPendingWork` → same-named async methods
//! - `ulid.Make().String()` → [`new_event_id`] (`patchbay_util::new_ulid()`)
//! - `taskAvailableFrame` / `runtimeProfilesChangedFrame` /
//!   `workspacesChangedFrame` / `pendingWorkFrame` → re-exported from
//!   `hub.rs` (package-private in Go, pub(crate) here)
//! - `M.WakeupPublishErrors` / `M.WakeupPublishedTotal` → [`crate::hub::M`]
//!
//! Port notes vs Go:
//! - Go's `RelayPublisher.PublishWithID` blocks the calling goroutine on Redis
//!   I/O; the Rust trait is async, so notifier methods are `async fn`.
//! - The event source stays behind the existing `patchbay_realtime::RelayPublisher`
//!   trait seam — service-layer callers depend on this type, not on any DB
//!   client.

use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use patchbay_realtime::{RelayPublisher, SCOPE_DAEMON_RUNTIME};

use crate::hub::{
    pending_work_frame, runtime_profiles_changed_frame, task_available_frame,
    workspaces_changed_frame, DaemonHub, M,
};

const DEFAULT_RELAY_PUBLISH_TIMEOUT: Duration = Duration::from_secs(2);

/// Sends daemon wakeup hints to the local daemon hub and, when Redis is
/// configured, publishes the same hint through the shared realtime relay so
/// every API node can attempt local delivery.
pub struct RelayNotifier {
    local: Option<Arc<DaemonHub>>,
    relay: RwLock<Option<Arc<dyn RelayPublisher>>>,
    publish_timeout: Duration,
}

impl RelayNotifier {
    /// `local`/`relay` are optional exactly like the Go nil-able fields.
    pub fn new(local: Option<Arc<DaemonHub>>, relay: Option<Arc<dyn RelayPublisher>>) -> Self {
        Self {
            local,
            relay: RwLock::new(relay),
            publish_timeout: DEFAULT_RELAY_PUBLISH_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_publish_timeout(mut self, timeout: Duration) -> Self {
        self.publish_timeout = timeout;
        self
    }

    /// Installs or removes the cross-node publisher without replacing the
    /// notifier shared by handler state and the task service.
    pub fn set_relay(&self, relay: Option<Arc<dyn RelayPublisher>>) {
        *self.relay.write().unwrap_or_else(|e| e.into_inner()) = relay;
    }

    fn relay(&self) -> Option<Arc<dyn RelayPublisher>> {
        self.relay.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub async fn notify_task_available(&self, runtime_id: &str, task_id: &str) {
        if runtime_id.is_empty() {
            return;
        }
        let event_id = new_event_id();
        if let Some(local) = &self.local {
            local.notify_task_available_with_event(runtime_id, task_id, &event_id);
        }
        let Some(relay) = self.relay() else {
            return;
        };
        let Some(frame) = task_available_frame(runtime_id, task_id) else {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let shard_key = if task_id.is_empty() {
            &event_id
        } else {
            task_id
        };
        if let Err(err) = self
            .publish_relay(&relay, shard_key, &frame, &event_id)
            .await
        {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                error = %err,
                runtime_id = %runtime_id,
                task_id = %task_id,
                "daemon websocket wakeup publish failed"
            );
            return;
        }
        M.wakeup_published_total.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn notify_runtime_profiles_changed(&self, workspace_id: &str, profile_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let event_id = new_event_id();
        if let Some(local) = &self.local {
            local.notify_runtime_profiles_changed_with_event(workspace_id, profile_id, &event_id);
        }
        let Some(relay) = self.relay() else {
            return;
        };
        let Some(frame) = runtime_profiles_changed_frame(workspace_id, profile_id) else {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Err(err) = self
            .publish_relay(&relay, workspace_id, &frame, &event_id)
            .await
        {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                error = %err,
                workspace_id = %workspace_id,
                runtime_profile_id = %profile_id,
                "daemon websocket profile refresh publish failed"
            );
            return;
        }
        M.wakeup_published_total.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn notify_workspaces_changed(&self, user_id: &str) {
        if user_id.is_empty() {
            return;
        }
        let event_id = new_event_id();
        if let Some(local) = &self.local {
            local.notify_workspaces_changed_with_event(user_id, &event_id);
        }
        let Some(relay) = self.relay() else {
            return;
        };
        let Some(frame) = workspaces_changed_frame() else {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        // SCOPE_DAEMON_RUNTIME is the relay's daemon-only transport scope; the
        // frame type tells Hub.DeliverDaemonRuntime whether scopeID is a
        // runtime, workspace, or user key. Keeping one transport scope
        // preserves compatibility with existing relay consumers while the hub
        // enforces user-scoped delivery.
        if let Err(err) = self.publish_relay(&relay, user_id, &frame, &event_id).await {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                error = %err,
                user_id = %user_id,
                "daemon websocket workspace refresh publish failed"
            );
            return;
        }
        M.wakeup_published_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Fans a runtime-scoped "heartbeat now" hint out to the local hub and,
    /// when Redis is configured, through the relay so the API node that
    /// actually holds the daemon's WebSocket delivers it (PB-5444). Shard key
    /// is the runtime ID: hints for one runtime stay ordered relative to each
    /// other, and a dropped hint only costs the daemon its normal heartbeat
    /// delay.
    pub async fn notify_pending_work(&self, runtime_id: &str, kind: &str) {
        if runtime_id.is_empty() {
            return;
        }
        let event_id = new_event_id();
        if let Some(local) = &self.local {
            local.notify_pending_work_with_event(runtime_id, kind, &event_id);
        }
        let Some(relay) = self.relay() else {
            return;
        };
        let Some(frame) = pending_work_frame(runtime_id, kind) else {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Err(err) = self
            .publish_relay(&relay, runtime_id, &frame, &event_id)
            .await
        {
            M.wakeup_publish_errors.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                error = %err,
                runtime_id = %runtime_id,
                kind = %kind,
                "daemon websocket pending work publish failed"
            );
            return;
        }
        M.wakeup_published_total.fetch_add(1, Ordering::Relaxed);
    }

    async fn publish_relay(
        &self,
        relay: &Arc<dyn RelayPublisher>,
        shard_key: &str,
        frame: &[u8],
        event_id: &str,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(
            self.publish_timeout,
            relay.publish_with_id(SCOPE_DAEMON_RUNTIME, shard_key, "", frame, event_id),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "relay publish timed out after {} ms",
                self.publish_timeout.as_millis()
            )
        })?
    }
}

/// Generates an event ID equivalent to Go's `ulid.Make().String()`: a 128-bit
/// ULID (48-bit unix-millisecond timestamp ‖ 80 random bits) encoded in
/// Crockford base32 — 26 uppercase characters. Event IDs only need uniqueness
/// (they key the per-client dedup LRU), but matching the Go wire/log format
/// keeps dashboards comparable across the cutover.
fn new_event_id() -> String {
    patchbay_util::new_ulid()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::test_support::{lock_metrics, reset_metrics};
    use patchbay_protocol::messages::{Message, TaskAvailablePayload};
    use patchbay_protocol::{
        EVENT_DAEMON_RUNTIME_PROFILES_CHANGED, EVENT_DAEMON_TASK_AVAILABLE,
        EVENT_DAEMON_WORKSPACES_CHANGED,
    };
    use std::sync::Mutex as StdMutex;
    use tokio::sync::mpsc;

    use async_trait::async_trait;

    #[derive(Clone, Default)]
    struct PublishRecord {
        scope_type: String,
        scope_id: String,
        exclude: String,
        frame: Vec<u8>,
        event_id: String,
    }

    /// Port of recordingRelayPublisher.
    struct RecordingRelayPublisher {
        records: StdMutex<Vec<PublishRecord>>,
        fail: bool,
    }

    impl RecordingRelayPublisher {
        fn new() -> Self {
            Self {
                records: StdMutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                records: StdMutex::new(Vec::new()),
                fail: true,
            }
        }

        fn records(&self) -> Vec<PublishRecord> {
            self.records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl RelayPublisher for RecordingRelayPublisher {
        async fn publish_with_id(
            &self,
            scope_type: &str,
            scope_id: &str,
            exclude: &str,
            frame: &[u8],
            event_id: &str,
        ) -> anyhow::Result<()> {
            if self.fail {
                return Err(anyhow::anyhow!("redis down"));
            }
            self.records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(PublishRecord {
                    scope_type: scope_type.to_string(),
                    scope_id: scope_id.to_string(),
                    exclude: exclude.to_string(),
                    frame: frame.to_vec(),
                    event_id: event_id.to_string(),
                });
            Ok(())
        }
    }

    struct PendingRelayPublisher;

    #[async_trait]
    impl RelayPublisher for PendingRelayPublisher {
        async fn publish_with_id(
            &self,
            _scope_type: &str,
            _scope_id: &str,
            _exclude: &str,
            _frame: &[u8],
            _event_id: &str,
        ) -> anyhow::Result<()> {
            std::future::pending().await
        }
    }

    /// Port of localFirstDaemonRelayPublisher: asserts the local hub fanout
    /// happened BEFORE the relay publish by draining the client's queue here,
    /// then records the call.
    struct LocalFirstRelayPublisher {
        rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>>,
        records: StdMutex<Vec<PublishRecord>>,
    }

    impl LocalFirstRelayPublisher {
        fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
            Self {
                rx: Arc::new(tokio::sync::Mutex::new(rx)),
                records: StdMutex::new(Vec::new()),
            }
        }

        fn records(&self) -> Vec<PublishRecord> {
            self.records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        async fn assert_no_pending(&self, what: &str) {
            let mut rx = self.rx.lock().await;
            assert!(
                rx.try_recv().is_err(),
                "expected {what} to be deduped, got duplicate"
            );
        }
    }

    #[async_trait]
    impl RelayPublisher for LocalFirstRelayPublisher {
        async fn publish_with_id(
            &self,
            scope_type: &str,
            scope_id: &str,
            exclude: &str,
            frame: &[u8],
            event_id: &str,
        ) -> anyhow::Result<()> {
            let local_frame = {
                let mut rx = self.rx.lock().await;
                rx.try_recv()
                    .expect("expected local fanout to happen before relay publish")
            };
            drop(local_frame);
            self.records
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(PublishRecord {
                    scope_type: scope_type.to_string(),
                    scope_id: scope_id.to_string(),
                    exclude: exclude.to_string(),
                    frame: frame.to_vec(),
                    event_id: event_id.to_string(),
                });
            Ok(())
        }
    }

    fn decode_frame(frame: &[u8]) -> (String, serde_json::Value) {
        let msg: Message = serde_json::from_slice(frame).expect("test frame decodes");
        (msg.r#type, msg.payload)
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn publishes_task_available_with_task_shard_key() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let relay = Arc::new(RecordingRelayPublisher::new());
        let notifier = RelayNotifier::new(None, Some(relay.clone()));

        notifier.notify_task_available("runtime-1", "task-1").await;

        let records = relay.records();
        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert_eq!(rec.scope_type, SCOPE_DAEMON_RUNTIME);
        assert_eq!(rec.scope_id, "task-1", "want task_id shard key");
        assert!(!rec.event_id.is_empty(), "expected event id");
        assert_eq!(rec.exclude, "");
        assert_eq!(M.wakeup_published_total.load(Ordering::Relaxed), 1);

        let (r#type, payload) = decode_frame(&rec.frame);
        assert_eq!(r#type, EVENT_DAEMON_TASK_AVAILABLE);
        let payload: TaskAvailablePayload = serde_json::from_value(payload).expect("payload");
        assert_eq!(payload.runtime_id, "runtime-1");
        assert_eq!(payload.task_id, "task-1");
    }

    #[tokio::test]
    async fn unresponsive_relay_is_bounded_and_counted() {
        let _guard = lock_metrics().await;
        reset_metrics();
        let hub = Arc::new(DaemonHub::new());
        let notifier = RelayNotifier::new(Some(hub), Some(Arc::new(PendingRelayPublisher)))
            .with_publish_timeout(Duration::ZERO);

        notifier.notify_task_available("runtime-1", "task-1").await;

        assert_eq!(M.wakeup_publish_errors.load(Ordering::Relaxed), 1);
        assert_eq!(M.wakeup_published_total.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn publishes_runtime_profiles_changed_with_workspace_shard_key() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let relay = Arc::new(RecordingRelayPublisher::new());
        let notifier = RelayNotifier::new(None, Some(relay.clone()));

        notifier
            .notify_runtime_profiles_changed("ws-1", "profile-1")
            .await;

        let records = relay.records();
        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert_eq!(rec.scope_type, SCOPE_DAEMON_RUNTIME);
        assert_eq!(rec.scope_id, "ws-1", "want workspace shard key");
        assert!(!rec.event_id.is_empty());
        assert_eq!(M.wakeup_published_total.load(Ordering::Relaxed), 1);

        let (r#type, payload) = decode_frame(&rec.frame);
        assert_eq!(r#type, EVENT_DAEMON_RUNTIME_PROFILES_CHANGED);
        assert_eq!(payload["workspace_id"], "ws-1");
        assert_eq!(payload["runtime_profile_id"], "profile-1");
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn publishes_workspaces_changed_with_user_shard_key() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let relay = Arc::new(RecordingRelayPublisher::new());
        let notifier = RelayNotifier::new(None, Some(relay.clone()));

        notifier.notify_workspaces_changed("user-1").await;

        let records = relay.records();
        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert_eq!(rec.scope_type, SCOPE_DAEMON_RUNTIME);
        assert_eq!(rec.scope_id, "user-1", "want user shard key");
        assert!(!rec.event_id.is_empty());

        let (r#type, _) = decode_frame(&rec.frame);
        assert_eq!(r#type, EVENT_DAEMON_WORKSPACES_CHANGED);
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn empty_keys_are_noops() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let relay = Arc::new(RecordingRelayPublisher::new());
        let notifier = RelayNotifier::new(None, Some(relay.clone()));

        notifier.notify_task_available("", "task-1").await;
        notifier
            .notify_runtime_profiles_changed("", "profile-1")
            .await;
        notifier.notify_workspaces_changed("").await;
        notifier.notify_pending_work("", "model_list").await;

        assert!(relay.records().is_empty(), "no publish for empty keys");
        assert_eq!(M.wakeup_published_total.load(Ordering::Relaxed), 0);
        assert_eq!(M.wakeup_publish_errors.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn publish_failure_counts_errors_and_skips_published_total() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let relay = Arc::new(RecordingRelayPublisher::failing());
        let notifier = RelayNotifier::new(None, Some(relay));

        notifier.notify_task_available("runtime-1", "task-1").await;

        assert_eq!(M.wakeup_publish_errors.load(Ordering::Relaxed), 1);
        assert_eq!(M.wakeup_published_total.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn task_shard_key_falls_back_to_event_id_when_task_missing() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let relay = Arc::new(RecordingRelayPublisher::new());
        let notifier = RelayNotifier::new(None, Some(relay.clone()));

        notifier.notify_task_available("runtime-1", "").await;

        let records = relay.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].scope_id, records[0].event_id,
            "empty task_id shards by event id"
        );
    }

    // ---- local/Redis loopback dedup (TestRelayNotifierDedups*Loopback) ------

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn dedups_local_redis_loopback_for_task_available() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let hub = Arc::new(DaemonHub::new());
        let (_client, rx) = hub.register(crate::hub::ClientIdentity {
            runtime_ids: vec!["runtime-1".into()],
            ..Default::default()
        });
        let relay = Arc::new(LocalFirstRelayPublisher::new(rx));
        let notifier = RelayNotifier::new(Some(hub.clone()), Some(relay.clone()));

        notifier.notify_task_available("runtime-1", "task-1").await;

        let records = relay.records();
        assert_eq!(records.len(), 1, "expected relay publish to be invoked");
        assert!(!records[0].event_id.is_empty(), "expected event id");
        assert_eq!(
            M.wakeup_delivered_hit.load(Ordering::Relaxed),
            1,
            "local delivery counts as a hit"
        );

        // Redis loopback of the SAME event id must be deduped per-client.
        let rec = &records[0];
        hub.deliver_daemon_runtime(&rec.scope_id, &rec.frame, &rec.event_id);
        relay.assert_no_pending("redis loopback").await;
        assert_eq!(M.wakeup_delivered_hit.load(Ordering::Relaxed), 1);
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn dedups_local_redis_loopback_for_runtime_profiles_changed() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let hub = Arc::new(DaemonHub::new());
        let (_client, rx) = hub.register(crate::hub::ClientIdentity {
            workspace_ids: vec!["ws-1".into()],
            ..Default::default()
        });
        let relay = Arc::new(LocalFirstRelayPublisher::new(rx));
        let notifier = RelayNotifier::new(Some(hub.clone()), Some(relay.clone()));

        notifier
            .notify_runtime_profiles_changed("ws-1", "profile-1")
            .await;

        let records = relay.records();
        assert_eq!(records.len(), 1, "expected relay publish to be invoked");
        assert!(!records[0].event_id.is_empty());
        assert_eq!(
            M.wakeup_delivered_hit.load(Ordering::Relaxed),
            0,
            "profile refresh delivery is not counted as a wakeup hit"
        );

        let rec = &records[0];
        hub.deliver_daemon_runtime(&rec.scope_id, &rec.frame, &rec.event_id);
        relay.assert_no_pending("redis loopback").await;
        assert_eq!(M.wakeup_delivered_hit.load(Ordering::Relaxed), 0);
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    // Serial metrics lock is intentionally held across awaits in this test.
    #[allow(clippy::await_holding_lock)]
    async fn dedups_local_redis_loopback_for_workspaces_changed() {
        let _guard = lock_metrics().await;
        reset_metrics();

        let hub = Arc::new(DaemonHub::new());
        let (_client, rx) = hub.register(crate::hub::ClientIdentity {
            user_id: "user-1".into(),
            ..Default::default()
        });
        let relay = Arc::new(LocalFirstRelayPublisher::new(rx));
        let notifier = RelayNotifier::new(Some(hub.clone()), Some(relay.clone()));

        notifier.notify_workspaces_changed("user-1").await;

        let records = relay.records();
        assert_eq!(
            records.len(),
            1,
            "expected local delivery followed by relay publish"
        );

        let rec = &records[0];
        hub.deliver_daemon_runtime(&rec.scope_id, &rec.frame, &rec.event_id);
        relay.assert_no_pending("redis loopback").await;
    }

    #[test]
    fn event_ids_are_crockford_ulids() {
        let ids: Vec<String> = (0..8).map(|_| new_event_id()).collect();
        for id in &ids {
            assert_eq!(id.len(), 26, "ULIDs are 26 chars: {id}");
            assert!(
                id.chars()
                    .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
                "{id}"
            );
        }
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "ids must be unique");
    }
}
