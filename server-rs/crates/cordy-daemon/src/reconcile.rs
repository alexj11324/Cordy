//! Port of `server/internal/daemon/reconcile.go` (lines 1–129).
//!
//! [`ReconcileBroadcaster`] fans out a "reconcile now" signal using the
//! close-and-replace channel pattern: daemon loops on coarse tickers get
//! nudged the moment the WS connection reconnects, without disturbing their
//! cadence. Edge-triggered with one-slot replay and a debounce window.
//!
//! Deviations from Go:
//! - Go's closed-channel snapshot → `tokio::sync::watch` generation counter:
//!   `notify()` clones a receiver that has "seen" the current generation, so
//!   it wakes exactly on the next `broadcast()` — the same edge-triggered
//!   semantics as parking on Go's pre-close channel.
//! - The replayed event is an immediately-ready snapshot instead of an
//!   already-closed channel.
//! - The injected test clock returns milliseconds since an arbitrary epoch
//!   (`u64`) rather than `time.Time`, so tests can pin the debounce window
//!   deterministically.
//! - `workspaceChangeSignal` uses a capacity-1 `tokio::sync::mpsc` channel;
//!   Go's nil-receiver tolerance becomes caller-side `Option`.

// S9-integration: consumed by hub.rs reconnect path + workspace sync loop
// wiring that lands with integration; silence dead-code until then.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

/// Milliseconds since an arbitrary epoch; stands in for Go's injectable
/// `now func() time.Time`.
pub(crate) type NowFn = Arc<dyn Fn() -> u64 + Send + Sync>;

fn system_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A single-shot wake signal handed out by [`ReconcileBroadcaster::notify`].
pub(crate) enum Snapshot {
    /// Replayed missed broadcast: resolves immediately (Go's already-closed
    /// channel).
    Ready,
    /// Live subscription: resolves on the next broadcast (Go's shared
    /// pre-close channel).
    Live(watch::Receiver<u64>),
}

impl Snapshot {
    /// `<-ch` equivalent: resolves when this snapshot fires.
    pub(crate) async fn fired(self) {
        match self {
            Snapshot::Ready => {}
            Snapshot::Live(mut rx) => {
                // Err only when the sender is gone; treat as fired so a
                // shutting-down broadcaster never hangs a subscriber.
                let _ = rx.changed().await;
            }
        }
    }
}

/// `reconcileBroadcaster` (reconcile.go:35–44).
pub(crate) struct ReconcileBroadcaster {
    tx: watch::Sender<u64>,
    state: Mutex<BroadcastState>,
}

struct BroadcastState {
    pending: bool,
    last_broadcast: Option<u64>,
    min_broadcast_interval: Duration,
    now: NowFn,
}

impl Default for ReconcileBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconcileBroadcaster {
    /// `newReconcileBroadcaster` (reconcile.go:46–52): 1s debounce, wall clock.
    pub(crate) fn new() -> Self {
        let (tx, _rx) = watch::channel(0);
        Self {
            tx,
            state: Mutex::new(BroadcastState {
                pending: false,
                last_broadcast: None,
                min_broadcast_interval: Duration::from_secs(1),
                now: Arc::new(system_now_millis),
            }),
        }
    }

    /// Test-only clock injection (reconcile.go:41–43).
    #[cfg(test)]
    fn set_now(&self, now: NowFn) {
        self.state.lock().unwrap().now = now;
    }

    /// Disable the debounce window (tests).
    #[cfg(test)]
    fn disable_debounce(&self) {
        self.state.lock().unwrap().min_broadcast_interval = Duration::ZERO;
    }

    /// `notify` (reconcile.go:63–77): return a signal that fires on the next
    /// broadcast. If a broadcast arrived while nobody was subscribed, the
    /// returned snapshot is already fired — replay observed exactly once.
    ///
    /// Subscribers must call `notify()` again after waking to receive the
    /// next signal; snapshots are single-shot.
    pub(crate) fn notify(&self) -> Snapshot {
        let mut state = self.state.lock().unwrap();
        if state.pending {
            // Replay the missed broadcast exactly once without disturbing
            // live subscribers, then resume edge-triggered behaviour.
            state.pending = false;
            return Snapshot::Ready;
        }
        Snapshot::Live(self.tx.subscribe())
    }

    /// `broadcast` (reconcile.go:86–98): wake every current subscriber and
    /// install a fresh generation for future ones. With no current
    /// subscribers, sets the one-slot replay flag. Calls within
    /// `min_broadcast_interval` of the previous broadcast are dropped;
    /// returns whether the signal fired.
    pub(crate) fn broadcast(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let now = (state.now)();
        if let Some(last) = state.last_broadcast {
            if now.saturating_sub(last) < state.min_broadcast_interval.as_millis() as u64 {
                return false;
            }
        }
        state.last_broadcast = Some(now);
        state.pending = true;
        // Wake all live receivers (those still holding the old generation).
        self.tx.send_modify(|gen| *gen += 1);
        true
    }
}

/// `workspaceChangeSignal` (reconcile.go:104–106): single-consumer dirty flag
/// for workspace-set changes. Capacity-1 buffer preserves an event arriving
/// before the sync loop starts while coalescing bursts into one trailing
/// reconciliation.
#[derive(Clone)]
pub(crate) struct WorkspaceChangeSignal {
    tx: tokio::sync::mpsc::Sender<()>,
    rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<()>>>,
}

impl Default for WorkspaceChangeSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceChangeSignal {
    /// `newWorkspaceChangeSignal` (reconcile.go:108–110).
    pub(crate) fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        Self {
            tx,
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
        }
    }

    /// `broadcast` (reconcile.go:119–128): non-blocking send; false when the
    /// slot is already dirty (coalesced).
    pub(crate) fn broadcast(&self) -> bool {
        self.tx.try_send(()).is_ok()
    }

    /// Receive of `s.notify()`'s channel: resolves once per recorded change.
    pub(crate) async fn wait(&self) {
        let mut rx = self.rx.lock().await;
        // Err only when the sender is gone; treat as fired.
        let _ = rx.recv().await;
    }

    /// Non-blocking poll of the flag (test convenience).
    #[cfg(test)]
    fn try_wait(&self) -> bool {
        self.rx
            .try_lock()
            .map(|mut rx| rx.try_recv().is_ok())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Fake clock backed by an atomic counter (reconcile_test.go's injected
    /// `b.now`).
    struct FakeClock(Arc<AtomicU64>);
    impl FakeClock {
        fn new(start_ms: u64) -> Self {
            Self(Arc::new(AtomicU64::new(start_ms)))
        }
        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
        fn now_fn(&self) -> NowFn {
            let counter = self.0.clone();
            Arc::new(move || counter.load(Ordering::SeqCst))
        }
    }

    /// TestReconcileBroadcaster_FansOutToManySubscribers (reconcile_test.go:12–46).
    #[tokio::test]
    async fn fans_out_to_many_subscribers() {
        let b = Arc::new(ReconcileBroadcaster::new());
        b.disable_debounce();

        const SUBS: usize = 16;
        let mut handles = Vec::with_capacity(SUBS);
        for _ in 0..SUBS {
            let snap = b.notify();
            handles.push(tokio::spawn(async move {
                snap.fired().await;
            }));
        }
        // Give subscribers a moment to park.
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(b.broadcast(), "broadcast() = false, want true");

        let joined = futures_util::future::join_all(handles);
        tokio::time::timeout(Duration::from_secs(2), joined)
            .await
            .expect("not all subscribers woke up");
    }

    /// TestReconcileBroadcaster_ReplaysMissedBroadcastToFirstLateSubscriber
    /// (reconcile_test.go:54–79).
    #[tokio::test]
    async fn replays_missed_broadcast_to_first_late_subscriber() {
        let b = ReconcileBroadcaster::new();
        b.disable_debounce();

        // No subscribers yet — fire.
        assert!(b.broadcast());

        // First late subscriber must see the replay.
        let first = b.notify();
        tokio::time::timeout(Duration::from_secs(1), first.fired())
            .await
            .expect("first late subscriber did not receive replayed broadcast");

        // Second late subscriber must NOT see the same replay; it parks.
        let second = b.notify();
        let result = tokio::time::timeout(Duration::from_millis(50), second.fired()).await;
        assert!(
            result.is_err(),
            "second late subscriber received a stale replay"
        );
    }

    /// TestReconcileBroadcaster_ReplayPersistsAcrossSubscriberDelay
    /// (reconcile_test.go:84–97).
    #[tokio::test]
    async fn replay_persists_across_subscriber_delay() {
        let b = ReconcileBroadcaster::new();
        b.disable_debounce();

        b.broadcast();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let ch = b.notify();
        tokio::time::timeout(Duration::from_secs(1), ch.fired())
            .await
            .expect("pending replay was lost after 100ms delay");
    }

    /// TestReconcileBroadcaster_DebouncesFlappingReconnects
    /// (reconcile_test.go:103–127).
    #[tokio::test]
    async fn debounces_flapping_reconnects() {
        let b = ReconcileBroadcaster::new();
        let clock = FakeClock::new(1_700_000_000_000);
        b.set_now(clock.now_fn());
        // min interval stays at the production default of 1s.

        assert!(b.broadcast(), "first broadcast suppressed");

        // Ten back-to-back calls within 900ms — all suppressed.
        for i in 1..=10 {
            clock.advance(90);
            assert!(!b.broadcast(), "broadcast at +{}ms not debounced", i * 90);
        }

        // Cross the threshold — next broadcast fires.
        clock.advance(1000);
        assert!(b.broadcast(), "broadcast past debounce window suppressed");
    }

    /// TestReconcileBroadcaster_DebounceBoundaryIsExact
    /// (reconcile_test.go:132–154): strict less-than comparison.
    #[tokio::test]
    async fn debounce_boundary_is_exact() {
        let b = ReconcileBroadcaster::new();
        let clock = FakeClock::new(1_700_000_000_000);
        b.set_now(clock.now_fn());

        assert!(b.broadcast(), "first broadcast suppressed");

        // Exactly at the interval — must fire (>= boundary allowed).
        clock.advance(1000);
        assert!(
            b.broadcast(),
            "broadcast at exact debounce boundary was suppressed"
        );

        // Just below the next boundary — must be suppressed.
        clock.advance(999);
        assert!(
            !b.broadcast(),
            "broadcast at boundary-minus-1ms was not suppressed"
        );
    }

    /// TestReconcileBroadcaster_ReSubscribesEachWake
    /// (reconcile_test.go:160–180).
    #[tokio::test]
    async fn re_subscribes_each_wake() {
        let b = ReconcileBroadcaster::new();
        b.disable_debounce();

        let snap1 = b.notify();
        b.broadcast();
        tokio::time::timeout(Duration::from_secs(1), snap1.fired())
            .await
            .expect("first wake did not arrive");

        // Broadcasting again must not panic; every broadcast sets the pending
        // replay flag, so the first notify() after it fires immediately.
        b.broadcast();
        let snap2 = b.notify();
        tokio::time::timeout(Duration::from_millis(50), snap2.fired())
            .await
            .expect("post-broadcast notify should observe the replay");

        // The next one is edge-triggered again: parks until a new broadcast.
        let snap3 = b.notify();
        let result = tokio::time::timeout(Duration::from_millis(50), snap3.fired()).await;
        assert!(
            result.is_err(),
            "second post-broadcast notify should be edge-triggered"
        );
    }

    /// TestWorkspaceChangeSignalCoalescesUntilConsumed (reconcile_test.go:182–199).
    #[tokio::test]
    async fn workspace_change_signal_coalesces_until_consumed() {
        let s = WorkspaceChangeSignal::new();
        assert!(s.broadcast(), "first workspace change was not recorded");
        assert!(
            !s.broadcast(),
            "duplicate workspace change should coalesce while dirty"
        );

        tokio::time::timeout(Duration::from_secs(1), s.wait())
            .await
            .expect("recorded workspace change was not delivered");
        assert!(!s.try_wait(), "signal should be empty after consumption");
        assert!(
            s.broadcast(),
            "workspace change after consumption was not recorded"
        );
    }

    /// TestReconcileBroadcaster_ConcurrentBroadcastAndNotify
    /// (reconcile_test.go:204–260): heavy concurrent traffic must converge.
    #[tokio::test]
    async fn concurrent_broadcast_and_notify() {
        let b = Arc::new(ReconcileBroadcaster::new());
        b.disable_debounce();

        let stop = Arc::new(tokio::sync::Notify::new());
        let stop_fired = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();

        // 8 subscribers in tight loops.
        for _ in 0..8 {
            let b = b.clone();
            let stop = stop.clone();
            let stop_fired = stop_fired.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    if stop_fired.load(Ordering::SeqCst) > 0 {
                        return;
                    }
                    let snap = b.notify();
                    tokio::select! {
                        _ = snap.fired() => {}
                        _ = stop.notified() => return,
                    }
                }
            }));
        }

        // 4 broadcasters in tight loops.
        for _ in 0..4 {
            let b = b.clone();
            let stop_fired = stop_fired.clone();
            handles.push(tokio::spawn(async move {
                while stop_fired.load(Ordering::SeqCst) == 0 {
                    b.broadcast();
                    tokio::task::yield_now().await;
                }
            }));
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        stop_fired.store(1, Ordering::SeqCst);
        stop.notify_waiters();

        // Bound the join — deadlock surfaces as a timeout failure.
        tokio::time::timeout(
            Duration::from_secs(2),
            futures_util::future::join_all(handles),
        )
        .await
        .expect("concurrent broadcast/notify did not converge after stop");
    }
}
