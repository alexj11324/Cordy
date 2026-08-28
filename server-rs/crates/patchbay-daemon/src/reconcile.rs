//! Reconciliation broadcasts and workspace change signals.
//!
//! Symbol map (Go → Rust):
//! - `reconcileBroadcaster` → [`ReconcileBroadcaster`]
//! - `newReconcileBroadcaster` → [`ReconcileBroadcaster::new`]
//! - `notify` / `broadcast` → same-named methods
//! - `workspaceChangeSignal` → [`WorkspaceChangeSignal`]
//!
//! Port notes: Go's close-and-replace channel pattern maps to a
//! `tokio::sync::watch`-free hand-rolled equivalent — the snapshot is an
//! `Arc<Notify>` pair where broadcast closes the current generation. The
//! closest faithful primitive here is a shared
//! `tokio::sync::broadcast`-like generation counter: subscribers hold a
//! [`Snapshot`] future that resolves when their generation is closed.
//! Implementation: each generation is a `tokio_util::sync::CancellationToken`;
//! `close(ch)` becomes `token.cancel()`, and the replaced channel is simply a
//! fresh token handed out by the next [`ReconcileBroadcaster::notify`] call.

use std::sync::Mutex;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

/// A subscription to one broadcast generation: resolves (once) on the next
/// broadcast — Go's receive from the closed `<-chan struct{}`.
#[derive(Clone)]
pub struct Snapshot {
    token: CancellationToken,
}

impl Snapshot {
    /// Waits for this generation's broadcast (`<-ch`).
    pub async fn recv(&self) {
        self.token.cancelled().await;
    }

    /// Whether the missed event already fired (`ch == nil`-style fast check);
    /// used by tests and non-blocking pollers.
    pub fn is_closed(&self) -> bool {
        self.token.is_cancelled()
    }
}

struct Inner {
    ch: CancellationToken,
    pending: bool,
    last_broadcast: Option<Instant>,
    min_broadcast_interval: std::time::Duration,
    /// Injected in tests to make the debounce window deterministic; production
    /// uses [`Instant::now`].
    now: fn() -> Instant,
}

/// `reconcileBroadcaster`: fans out a "reconcile now" signal to any number of
/// listeners using the close-and-replace channel pattern. Subscribers call
/// [`Self::notify`] to obtain a snapshot that fires on the next broadcast;
/// after waking, a subscriber re-acquires a fresh snapshot via notify() to
/// receive the next signal.
///
/// The broadcaster exists because some daemon loops run on coarse tickers
/// (task-cancellation polling at 5s, workspace sync at 30s). When the daemon's
/// WS connection drops and reconnects, anything the server changed during the
/// gap is invisible to those loops until their next tick fires. [`Self::broadcast`]
/// lets the WS connect path nudge every waiter to re-check immediately,
/// without disturbing the ticker cadence.
///
/// Edge-triggered with one-slot replay: if broadcast() fires while nobody is
/// subscribed, the next notify() call returns an already-fired snapshot so the
/// late subscriber observes the missed event exactly once.
///
/// Broadcast calls within `min_broadcast_interval` of the previous one are
/// debounced, so a flapping WS connection cannot translate a network blip into
/// a stampede of GetTaskStatus / ListWorkspaces requests.
pub struct ReconcileBroadcaster {
    inner: Mutex<Inner>,
}

impl ReconcileBroadcaster {
    /// `newReconcileBroadcaster`.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                ch: CancellationToken::new(),
                pending: false,
                last_broadcast: None,
                min_broadcast_interval: std::time::Duration::from_secs(1),
                now: Instant::now,
            }),
        }
    }

    /// Test-only constructor with an injected debounce window.
    #[cfg(test)]
    pub(crate) fn with_interval(min_broadcast_interval: std::time::Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                ch: CancellationToken::new(),
                pending: false,
                last_broadcast: None,
                min_broadcast_interval,
                now: Instant::now,
            }),
        }
    }

    /// `notify`: returns a snapshot that fires on the next broadcast.
    /// Subscribers should call notify() again after waking to receive the next
    /// signal — the returned snapshot is single-shot.
    ///
    /// If a broadcast arrived while there were no subscribers, the snapshot
    /// returned by the next notify() call is already fired. The replay flag is
    /// cleared by that call: a second concurrent late subscriber does NOT see
    /// the same replayed event.
    pub fn notify(&self) -> Snapshot {
        let mut inner = self.inner.lock().unwrap();
        if inner.pending {
            // Replay the missed broadcast exactly once: hand back a fresh,
            // already-fired token without disturbing b.ch for real
            // subscribers, and clear pending so the next notify() resumes
            // edge-triggered behaviour.
            inner.pending = false;
            let replay = CancellationToken::new();
            replay.cancel();
            return Snapshot { token: replay };
        }
        Snapshot {
            token: inner.ch.clone(),
        }
    }

    /// `broadcast`: wakes every current subscriber, then installs a fresh
    /// generation for future subscribers. If there are no current subscribers,
    /// a one-slot replay flag is set so the next notify() observes the missed
    /// event once. Reports whether the signal fired so callers can log
    /// debug-level traces of suppressed broadcasts.
    pub fn broadcast(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let now = (inner.now)();
        if let Some(last) = inner.last_broadcast {
            if now.duration_since(last) < inner.min_broadcast_interval {
                return false;
            }
        }
        inner.last_broadcast = Some(now);
        inner.pending = true;
        inner.ch.cancel();
        inner.ch = CancellationToken::new();
        true
    }
}

impl Default for ReconcileBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// `workspaceChangeSignal`: a single-consumer dirty flag for workspace-set
/// changes. The one-slot buffer preserves an event that arrives before the sync
/// loop starts or while an API read is in flight, while coalescing any larger
/// burst into one trailing reconciliation of the latest server state.
///
/// Go's buffered `<-chan struct{}` (capacity 1) becomes a `tokio::sync::mpsc`
/// capacity-1 channel; receiving from it is [`Self::recv`].
pub struct WorkspaceChangeSignal {
    tx: tokio::sync::mpsc::Sender<()>,
    rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<()>>,
}

impl Default for WorkspaceChangeSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceChangeSignal {
    /// `newWorkspaceChangeSignal`.
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        Self {
            tx,
            rx: tokio::sync::Mutex::new(rx),
        }
    }

    /// `notify` + receive: waits for one coalesced change event (Go's
    /// `<-s.ch`). Returns None only if the sender side was dropped.
    pub async fn recv(&self) -> Option<()> {
        self.rx.lock().await.recv().await
    }

    /// `broadcast`: enqueues one dirty mark; a full buffer means an event is
    /// already pending (coalesced), which reports false like Go's default arm.
    pub fn broadcast(&self) -> bool {
        self.tx.try_send(()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn broadcast_wakes_subscribers() {
        let b = ReconcileBroadcaster::new();
        let snap = b.notify();
        assert!(!snap.is_closed());
        assert!(b.broadcast());
        assert!(snap.is_closed());
    }

    #[test]
    fn replay_slot_fires_once_for_late_subscriber() {
        let b = ReconcileBroadcaster::new();
        assert!(b.broadcast()); // nobody subscribed
        let first = b.notify();
        assert!(
            first.is_closed(),
            "late subscriber observes the missed event"
        );
        let second = b.notify();
        assert!(!second.is_closed(), "replay fires exactly once");
    }

    #[test]
    fn debounce_drops_back_to_back_broadcasts() {
        let b = ReconcileBroadcaster::with_interval(Duration::from_secs(60));
        assert!(b.broadcast());
        assert!(!b.broadcast(), "inside min interval → dropped");
        // After the window elapses the next broadcast fires again.
        b.inner.lock().unwrap().last_broadcast = Some(Instant::now() - Duration::from_secs(61));
        assert!(b.broadcast());
    }

    #[tokio::test]
    async fn snapshot_recv_resolves_on_broadcast() {
        let b = std::sync::Arc::new(ReconcileBroadcaster::new());
        let snap = b.notify();
        let b2 = b.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            b2.broadcast();
        });
        tokio::time::timeout(Duration::from_secs(1), snap.recv())
            .await
            .expect("broadcast resolves the snapshot");
    }

    #[test]
    fn workspace_change_signal_coalesces() {
        let s = WorkspaceChangeSignal::new();
        assert!(s.broadcast());
        assert!(!s.broadcast(), "one-slot buffer full → coalesced");
        // Draining reopens the slot.
        s.rx.blocking_lock().blocking_recv();
        assert!(s.broadcast());
    }
}
