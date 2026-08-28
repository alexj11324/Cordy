//! Managed-relay lifecycle and the mirrored dual-write rollout helper —

use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::broadcaster::{
    Broadcaster, DaemonRuntimeDeliverer, RelayPublisher, SCOPE_DAEMON_RUNTIME, SCOPE_USER,
};
use crate::envelope::{inject_event_id, HubFanout};
use crate::metrics::M;

/// Redis-backed realtime relay with explicit task lifecycle management.
///
/// `start` spawns background consumer tasks which shut down when the passed
/// [`CancellationToken`] fires (Go passes a `context.Context`); `stop`
/// signals and [`ManagedRelay::wait`] joins.
#[async_trait]
pub trait ManagedRelay: RelayPublisher + Broadcaster {
    fn node_id(&self) -> String;

    /// Starts background consumer tasks; they stop when `shutdown` fires.
    fn start(self: Arc<Self>, shutdown: CancellationToken);

    /// Signals background tasks to stop.
    fn stop(&self);

    /// Joins background tasks after [`ManagedRelay::stop`].
    async fn wait(&self);

    /// Optional hook: registers a deliverer for daemon-runtime scoped frames.
    /// Default no-op; relays that support daemon-runtime fanout override it
    /// (Go uses an optional interface upgrade at the call site).
    fn set_daemon_runtime_deliverer(&self, _deliverer: Arc<dyn DaemonRuntimeDeliverer>) {}
}

/// Local-first broadcaster whose Redis relay can attach after startup.
///
/// The server always installs this stable producer handle. If Redis is down at
/// boot, events still reach local clients while a supervisor retries relay
/// construction. Once attached, the same handle dual-writes without requiring
/// producers or event-bus listeners to be rebuilt.
pub struct SwitchableRelayBroadcaster {
    hub: Arc<dyn HubFanout>,
    relay: RwLock<Option<Arc<dyn ManagedRelay>>>,
}

impl SwitchableRelayBroadcaster {
    pub fn new(hub: Arc<dyn HubFanout>) -> Self {
        Self {
            hub,
            relay: RwLock::new(None),
        }
    }

    pub fn set_relay(&self, relay: Option<Arc<dyn ManagedRelay>>) {
        *self.relay.write().unwrap_or_else(|e| e.into_inner()) = relay;
    }

    pub fn relay(&self) -> Option<Arc<dyn ManagedRelay>> {
        self.relay.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    async fn deliver_and_publish(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        message: &[u8],
    ) {
        let Some(relay) = self.relay() else {
            if scope_type == "global" {
                self.hub.fanout_all_dedup(message, exclude, "").await;
            } else if scope_type == SCOPE_USER {
                self.hub.fanout_user(scope_id, message, exclude, "").await;
            } else {
                self.hub
                    .broadcast_to_scope_dedup(scope_type, scope_id, message, "")
                    .await;
            }
            return;
        };

        let event_id = patchbay_util::new_ulid();
        let frame = inject_event_id(message, &event_id);
        if scope_type == "global" {
            self.hub.fanout_all_dedup(&frame, exclude, &event_id).await;
        } else if scope_type == SCOPE_USER {
            self.hub
                .fanout_user(scope_id, &frame, exclude, &event_id)
                .await;
        } else {
            self.hub
                .broadcast_to_scope_dedup(scope_type, scope_id, &frame, &event_id)
                .await;
        }
        if let Err(error) = relay
            .publish_with_id(scope_type, scope_id, exclude, &frame, &event_id)
            .await
        {
            tracing::warn!(
                %error,
                scope = scope_type,
                scope_id,
                event_id,
                "realtime relay publish failed after local delivery"
            );
        }
    }
}

#[async_trait]
impl Broadcaster for SwitchableRelayBroadcaster {
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]) {
        self.deliver_and_publish(scope_type, scope_id, "", message)
            .await;
    }

    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>) {
        self.deliver_and_publish(
            SCOPE_USER,
            user_id,
            exclude_workspace.unwrap_or_default(),
            message,
        )
        .await;
    }

    async fn broadcast(&self, message: &[u8]) {
        self.deliver_and_publish("global", "all", "", message).await;
    }
}

/// Temporary rollout helper: starts two relay backends, reads from both, and
/// publishes every event to both with the same event id. Client-side dedup
/// keeps loopback delivery idempotent.
pub struct MirroredRelay {
    primary: Arc<dyn ManagedRelay>,
    mirror: Arc<dyn ManagedRelay>,
}

impl MirroredRelay {
    pub fn new(primary: Arc<dyn ManagedRelay>, mirror: Arc<dyn ManagedRelay>) -> Self {
        Self { primary, mirror }
    }

    pub fn set_daemon_runtime_deliverer(&self, d: Arc<dyn DaemonRuntimeDeliverer>) {
        self.primary.set_daemon_runtime_deliverer(d.clone());
        self.mirror.set_daemon_runtime_deliverer(d);
    }
}

#[async_trait]
impl RelayPublisher for MirroredRelay {
    async fn publish_with_id(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        frame: &[u8],
        event_id: &str,
    ) -> anyhow::Result<()> {
        let primary_res = self
            .primary
            .publish_with_id(scope_type, scope_id, exclude, frame, event_id)
            .await;
        // Daemon-runtime frames are consumed by the daemon hub wired to the
        // primary only — mirroring them would double-deliver wakeups.
        if scope_type == SCOPE_DAEMON_RUNTIME {
            return primary_res;
        }
        let mirror_res = self
            .mirror
            .publish_with_id(scope_type, scope_id, exclude, frame, event_id)
            .await;

        let m = &*M;
        if let Err(e) = &primary_res {
            m.redis_mirror_primary_errors
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                error = %e,
                scope = %scope_type,
                scope_id = %scope_id,
                event_id = %event_id,
                "realtime/redis mirror: primary publish failed"
            );
        }
        if let Err(e) = &mirror_res {
            m.redis_mirror_secondary_errors
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                error = %e,
                scope = %scope_type,
                scope_id = %scope_id,
                event_id = %event_id,
                "realtime/redis mirror: secondary publish failed"
            );
        }
        if primary_res.is_ok() != mirror_res.is_ok() {
            m.redis_mirror_divergence_total
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                primary_error = ?primary_res.as_ref().err().map(|e| e.to_string()),
                secondary_error = ?mirror_res.as_ref().err().map(|e| e.to_string()),
                scope = %scope_type,
                scope_id = %scope_id,
                event_id = %event_id,
                "realtime/redis mirror: divergent publish result"
            );
        }

        // errors.Join equivalent: succeed only when both succeeded.
        match (&primary_res, &mirror_res) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(a), Ok(())) => Err(anyhow::anyhow!("{a:#}")),
            (Ok(()), Err(b)) => Err(anyhow::anyhow!("{b:#}")),
            (Err(a), Err(b)) => Err(anyhow::anyhow!("{a:#}; {b:#}")),
        }
    }
}

#[async_trait]
impl Broadcaster for MirroredRelay {
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]) {
        let id = patchbay_util::new_ulid();
        let _ = self
            .publish_with_id(scope_type, scope_id, "", message, &id)
            .await;
    }

    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>) {
        let exclude = exclude_workspace.unwrap_or("");
        let id = patchbay_util::new_ulid();
        let _ = self
            .publish_with_id(SCOPE_USER, user_id, exclude, message, &id)
            .await;
    }

    async fn broadcast(&self, message: &[u8]) {
        let id = patchbay_util::new_ulid();
        let _ = self
            .publish_with_id("global", "all", "", message, &id)
            .await;
    }

    // broadcast_to_workspace inherits the default SCOPE_WORKSPACE delegation.
}

#[async_trait]
impl ManagedRelay for MirroredRelay {
    fn node_id(&self) -> String {
        self.primary.node_id()
    }

    fn start(self: Arc<Self>, shutdown: CancellationToken) {
        self.primary.clone().start(shutdown.clone());
        self.mirror.clone().start(shutdown);
        *M.node_id.write().unwrap_or_else(|e| e.into_inner()) = self.node_id();
    }

    fn stop(&self) {
        self.primary.stop();
        self.mirror.stop();
    }

    async fn wait(&self) {
        self.primary.wait().await;
        self.mirror.wait().await;
    }

    fn set_daemon_runtime_deliverer(&self, deliverer: Arc<dyn DaemonRuntimeDeliverer>) {
        self.primary.set_daemon_runtime_deliverer(deliverer.clone());
        self.mirror.set_daemon_runtime_deliverer(deliverer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcaster::SCOPE_WORKSPACE;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    /// Serializes tests that touch the global metrics singleton `M` —
    /// concurrent `reset()` calls would wipe another test's counters
    /// between its increments and assertions. Async-aware so the guard may
    /// be held across `.await` in async tests.
    static METRICS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[derive(Default)]
    struct MockRelay {
        name: &'static str,
        fail_publish: AtomicBool,
        /// (scope_type, scope_id, event_id) per publish.
        published: Mutex<Vec<(String, String, String)>>,
        frames: Mutex<Vec<Vec<u8>>>,
        started: AtomicBool,
        stopped: AtomicBool,
        daemon_deliverer_set: AtomicBool,
    }

    impl MockRelay {
        fn new(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                ..Default::default()
            })
        }
    }

    #[async_trait]
    impl RelayPublisher for MockRelay {
        async fn publish_with_id(
            &self,
            scope_type: &str,
            scope_id: &str,
            _exclude: &str,
            _frame: &[u8],
            event_id: &str,
        ) -> anyhow::Result<()> {
            self.published.lock().unwrap().push((
                scope_type.to_string(),
                scope_id.to_string(),
                event_id.to_string(),
            ));
            self.frames.lock().unwrap().push(_frame.to_vec());
            if self.fail_publish.load(Ordering::Relaxed) {
                anyhow::bail!("{} publish failed", self.name);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl Broadcaster for MockRelay {
        async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, _m: &[u8]) {
            let _ = self
                .publish_with_id(scope_type, scope_id, "", b"m", "mock-id")
                .await;
        }
        async fn send_to_user(&self, user_id: &str, _m: &[u8], _exclude_workspace: Option<&str>) {
            let _ = self
                .publish_with_id(SCOPE_USER, user_id, "", b"m", "mock-id")
                .await;
        }
        async fn broadcast(&self, _m: &[u8]) {
            let _ = self
                .publish_with_id("global", "all", "", b"m", "mock-id")
                .await;
        }
    }

    #[async_trait]
    impl ManagedRelay for MockRelay {
        fn node_id(&self) -> String {
            format!("node-{}", self.name)
        }
        fn start(self: Arc<Self>, _shutdown: CancellationToken) {
            self.started.store(true, Ordering::Relaxed);
        }
        fn stop(&self) {
            self.stopped.store(true, Ordering::Relaxed);
        }
        async fn wait(&self) {}
        fn set_daemon_runtime_deliverer(&self, _deliverer: Arc<dyn DaemonRuntimeDeliverer>) {
            self.daemon_deliverer_set.store(true, Ordering::Relaxed);
        }
    }

    struct NoopDaemonDeliverer;

    impl DaemonRuntimeDeliverer for NoopDaemonDeliverer {
        fn deliver_daemon_runtime(&self, _scope_id: &str, _frame: &[u8], _event_id: &str) {}
    }

    #[tokio::test]
    async fn mirrored_relay_publishes_to_both_backends() {
        let primary = MockRelay::new("primary");
        let mirror = MockRelay::new("mirror");
        let relay = MirroredRelay::new(primary.clone(), mirror.clone());

        relay
            .publish_with_id(SCOPE_WORKSPACE, "ws-1", "", b"frame", "evt-1")
            .await
            .unwrap();

        assert_eq!(primary.published.lock().unwrap().len(), 1);
        assert_eq!(mirror.published.lock().unwrap().len(), 1);
        // Same event id on both backends — client-side dedup relies on it.
        assert_eq!(primary.published.lock().unwrap()[0].2, "evt-1");
        assert_eq!(mirror.published.lock().unwrap()[0].2, "evt-1");
    }

    #[tokio::test]
    async fn switchable_broadcaster_delivers_locally_before_relay_recovers() {
        let hub = Arc::new(crate::hub::Hub::new());
        let mut client = hub.register("user-1", "ws-1").1;
        let broadcaster = SwitchableRelayBroadcaster::new(hub);

        broadcaster
            .broadcast_to_scope(SCOPE_WORKSPACE, "ws-1", br#"{"type":"issue:updated"}"#)
            .await;

        assert_eq!(
            client.recv().await.as_deref(),
            Some(br#"{"type":"issue:updated"}"#.as_slice())
        );
    }

    #[tokio::test]
    async fn switchable_broadcaster_uses_one_event_id_for_local_and_relay_delivery() {
        let hub = Arc::new(crate::hub::Hub::new());
        let mut client = hub.register("user-1", "ws-1").1;
        let broadcaster = SwitchableRelayBroadcaster::new(hub);
        let relay = MockRelay::new("recovered");
        broadcaster.set_relay(Some(relay.clone()));

        broadcaster
            .broadcast_to_scope(SCOPE_WORKSPACE, "ws-1", br#"{"type":"issue:updated"}"#)
            .await;

        let local_frame = client.recv().await.expect("local delivery");
        let published = relay.published.lock().unwrap();
        let frames = relay.frames.lock().unwrap();
        assert_eq!(published.len(), 1);
        assert!(!published[0].2.is_empty());
        assert_eq!(frames.as_slice(), &[local_frame]);
        let decoded: serde_json::Value = serde_json::from_slice(&frames[0]).unwrap();
        assert_eq!(decoded["event_id"], published[0].2);
    }

    #[tokio::test]
    async fn daemon_runtime_scope_skips_mirror() {
        let primary = MockRelay::new("primary");
        let mirror = MockRelay::new("mirror");
        let relay = MirroredRelay::new(primary.clone(), mirror.clone());

        relay
            .publish_with_id(SCOPE_DAEMON_RUNTIME, "rt-1", "", b"wake", "evt-2")
            .await
            .unwrap();

        assert_eq!(primary.published.lock().unwrap().len(), 1);
        assert_eq!(mirror.published.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn join_reports_both_failures_and_tracks_metrics() {
        let _metrics_guard = METRICS_LOCK.lock().await;
        M.reset();
        let primary = MockRelay::new("primary");
        let mirror = MockRelay::new("mirror");
        primary.fail_publish.store(true, Ordering::Relaxed);
        mirror.fail_publish.store(true, Ordering::Relaxed);
        let relay = MirroredRelay::new(primary.clone(), mirror.clone());

        let err = relay
            .publish_with_id(SCOPE_WORKSPACE, "ws-1", "", b"f", "evt-3")
            .await
            .unwrap_err();

        assert!(err.to_string().contains("primary"));
        assert!(err.to_string().contains("mirror"));
        assert_eq!(M.redis_mirror_primary_errors.load(Ordering::Relaxed), 1);
        assert_eq!(M.redis_mirror_secondary_errors.load(Ordering::Relaxed), 1);
        assert_eq!(
            M.redis_mirror_divergence_total.load(Ordering::Relaxed),
            0,
            "both failed identically — not a divergence"
        );
        M.reset();
    }

    #[test]
    fn lifecycle_forwards_and_stamps_node_id() {
        let _metrics_guard = METRICS_LOCK.blocking_lock();
        M.reset();
        let primary = MockRelay::new("primary");
        let mirror = MockRelay::new("mirror");
        let relay = Arc::new(MirroredRelay::new(primary.clone(), mirror.clone()));

        relay.clone().start(CancellationToken::new());
        assert!(primary.started.load(Ordering::Relaxed));
        assert!(mirror.started.load(Ordering::Relaxed));
        assert_eq!(M.node_id.read().unwrap().as_str(), "node-primary");

        relay.stop();
        assert!(primary.stopped.load(Ordering::Relaxed));
        assert!(mirror.stopped.load(Ordering::Relaxed));
        M.reset();
    }

    #[test]
    fn managed_mirror_forwards_daemon_deliverer_to_children() {
        let primary = MockRelay::new("primary");
        let mirror = MockRelay::new("mirror");
        let relay: Arc<dyn ManagedRelay> =
            Arc::new(MirroredRelay::new(primary.clone(), mirror.clone()));

        relay.set_daemon_runtime_deliverer(Arc::new(NoopDaemonDeliverer));

        assert!(primary.daemon_deliverer_set.load(Ordering::Relaxed));
        assert!(mirror.daemon_deliverer_set.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn broadcaster_shortcuts_use_expected_scopes() {
        let primary = MockRelay::new("primary");
        let mirror = MockRelay::new("mirror");
        let relay = MirroredRelay::new(primary.clone(), mirror.clone());

        relay.broadcast_to_workspace("ws-9", b"m").await;
        assert_eq!(primary.published.lock().unwrap()[0].0, SCOPE_WORKSPACE);

        relay.send_to_user("user-7", b"m", Some("ws-9")).await;
        assert_eq!(
            primary.published.lock().unwrap().last().unwrap().0,
            SCOPE_USER
        );

        relay.broadcast(b"m").await;
        assert_eq!(
            primary.published.lock().unwrap().last().unwrap().0,
            "global"
        );
    }
}
