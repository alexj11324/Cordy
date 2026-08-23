//! Per-chat-session run-trigger debouncer.
//!
//! Port of `server/internal/integrations/channel/engine/batcher.go`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// The silence window the inbound debouncer waits before triggering an
/// agent run for a chat session. 3s (MUL-2968): long enough to absorb a
/// "forward a transcript, then type a note" burst into one run, short
/// enough that the bot's first reply is not perceptibly late.
pub const DEFAULT_CHAT_RUN_BATCH_WINDOW: Duration = Duration::from_secs(3);

/// Debounces the per-chat_session run trigger. Each inbound message that
/// lands in a session calls [`PendingBatcher::schedule`], which (re)arms a
/// single timer for that session; when the session goes quiet for the
/// window the latest flush runs exactly once. This collapses a burst into
/// ONE agent run — safe because the chat task reads the WHOLE session at
/// run time. Only the run TRIGGER is debounced; the chat_message rows,
/// dedup, and frame ACK already happened synchronously upstream.
///
/// State is in-process, keyed by chat_session_id (a globally-unique
/// UUID). The WS lease guarantees a single active owner per installation,
/// so a session is debounced by one process. A hard crash inside the
/// window drops the pending trigger (messages are durable; they just do
/// not fire a run until the next message). Graceful shutdown calls
/// [`PendingBatcher::flush_all`] so that boundary is not hit on a normal
/// restart. Task-safe; one instance is shared across supervisors.
///
/// Port note: Go arms `time.AfterFunc(d, fn)` per entry; Rust drives one
/// reaper task per batcher that sleeps until the earliest deadline. The
/// generation fencing (seq/gen) is preserved verbatim: a stale fire bails
/// when a newer schedule superseded it, closing the cancel-vs-fire race
/// the Go comment describes.
pub struct PendingBatcher {
    window: Duration,
    inner: Mutex<BatcherInner>,
    /// Signalled whenever an entry is armed, so the reaper recomputes its
    /// sleep deadline.
    armed: Notify,
    /// Set once flush_all runs; later schedules run inline.
    stopped: CancellationToken,
    /// Counts flushes currently executing outside the lock (the Go
    /// inflight WaitGroup).
    inflight: Mutex<usize>,
    inflight_zero: Notify,
    /// Ensures exactly one reaper task exists per batcher.
    reaper_spawned: AtomicBool,
}

struct BatcherInner {
    pending: HashMap<String, PendingEntry>,
    /// Monotonic generation minted per (re)schedule.
    seq: u64,
}

#[derive(Clone)]
struct PendingEntry {
    /// Cancellation for this entry's timer slot.
    cancel: CancellationToken,
    flush: Arc<dyn Fn() + Send + Sync>,
    /// The schedule generation this entry was armed with; a fire for a
    /// superseded generation is dropped by retain-on-deadline semantics
    /// below (the entry it belonged to no longer exists).
    #[allow(dead_code)]
    gen: u64,
    /// Instant the window elapses.
    deadline: std::time::Instant,
}

struct InflightGuard {
    batcher: std::sync::Weak<PendingBatcher>,
    count: usize,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        let Some(batch) = self.batcher.upgrade() else {
            return;
        };
        let mut inflight = batch.inflight.lock().unwrap_or_else(|e| e.into_inner());
        *inflight -= self.count;
        if *inflight == 0 {
            batch.inflight_zero.notify_waiters();
        }
    }
}

impl PendingBatcher {
    /// Returns a batcher with the given silence window. A zero window
    /// falls back to [`DEFAULT_CHAT_RUN_BATCH_WINDOW`] (Go treats any
    /// non-positive value the same way).
    pub fn new(window: Duration) -> Arc<Self> {
        let window = if window.is_zero() {
            DEFAULT_CHAT_RUN_BATCH_WINDOW
        } else {
            window
        };
        Arc::new(Self {
            window,
            inner: Mutex::new(BatcherInner {
                pending: HashMap::new(),
                seq: 0,
            }),
            armed: Notify::new(),
            stopped: CancellationToken::new(),
            inflight: Mutex::new(0),
            inflight_zero: Notify::new(),
            reaper_spawned: AtomicBool::new(false),
        })
    }

    /// The effective silence window (defaults applied).
    pub fn window(&self) -> Duration {
        self.window
    }

    /// (Re)arms the silence window for `key`. The most recent flush wins:
    /// only session-level information is needed to fire a run, so keeping
    /// the latest closure (which captures the latest installation/message
    /// context) suffices. Calling schedule after [`Self::flush_all`] runs
    /// the flush inline rather than dropping it (the shutdown race where
    /// a message arrives after the drain has begun).
    pub fn schedule<F: Fn() + Send + Sync + 'static>(self: &Arc<Self>, key: &str, flush: F) {
        let flush = Arc::new(flush);
        {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if self.stopped.is_cancelled() {
                drop(inner);
                flush();
                return;
            }
            inner.seq += 1;
            let gen = inner.seq;
            let deadline = std::time::Instant::now() + self.window;
            let entry = PendingEntry {
                cancel: CancellationToken::new(),
                flush: flush.clone(),
                gen,
                deadline,
            };
            if let Some(existing) = inner.pending.get_mut(key) {
                // Retire the superseded slot; its deadline can no longer
                // win the reaper's min() because the map holds only the
                // new one under the same key.
                existing.cancel.cancel();
                *existing = entry;
            } else {
                inner.pending.insert(key.to_string(), entry);
            }
        }
        self.armed.notify_one();
        self.spawn_reaper_if_needed();
    }

    /// Ensures exactly one reaper task drives all armed timers for this
    /// batcher. It sleeps until the earliest deadline, fires due entries,
    /// and parks on `armed` when nothing is pending.
    fn spawn_reaper_if_needed(self: &Arc<Self>) {
        if self.reaper_spawned.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(batch) = this.upgrade() else { return };
                if batch.stopped.is_cancelled() {
                    return;
                }
                let next_deadline = {
                    let inner = batch.inner.lock().unwrap();
                    inner.pending.values().map(|e| e.deadline).min()
                };
                match next_deadline {
                    None => {
                        // Park until the next arm (or batcher drop).
                        drop(batch);
                        park(&this).await;
                    }
                    Some(deadline) => {
                        let now = std::time::Instant::now();
                        if now >= deadline {
                            batch.fire_due(now);
                        } else {
                            let sleep = deadline - now;
                            drop(batch);
                            tokio::select! {
                                _ = tokio::time::sleep(sleep) => {}
                                _ = park(&this) => {}
                            }
                        }
                    }
                }
            }
        });
    }

    /// Fires every entry whose deadline has passed, outside the lock,
    /// exactly once per entry.
    fn fire_due(self: &Arc<Self>, now: std::time::Instant) {
        let due: Vec<Arc<dyn Fn() + Send + Sync>> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let mut due = Vec::new();
            inner.pending.retain(|_, entry| {
                if entry.deadline <= now {
                    due.push(entry.flush.clone());
                    false
                } else {
                    true
                }
            });
            due
        };
        if due.is_empty() {
            return;
        }
        let due_len = due.len();
        *self.inflight.lock().unwrap_or_else(|e| e.into_inner()) += due_len;
        let this = Arc::downgrade(self);
        tokio::spawn(async move {
            let _guard = InflightGuard {
                batcher: this,
                count: due_len,
            };
            for flush in due {
                flush();
            }
        });
    }

    /// Stops the batcher and runs every still-pending flush exactly once,
    /// then waits for concurrently-firing flushes to finish. Call once
    /// from graceful shutdown AFTER inbound delivery has stopped. After
    /// flush_all the batcher is terminal: later schedule calls run inline.
    pub async fn flush_all(&self) {
        let entries: Vec<Arc<dyn Fn() + Send + Sync>> = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            self.stopped.cancel();
            let mut entries = Vec::with_capacity(inner.pending.len());
            for (_, e) in inner.pending.drain() {
                e.cancel.cancel();
                entries.push(e.flush);
            }
            entries
        };
        for flush in entries {
            flush();
        }
        // Wait for concurrently-firing flushes (Go inflight.Wait()).
        while *self.inflight.lock().unwrap() != 0 {
            self.inflight_zero.notified().await;
        }
    }

    /// Reports how many sessions currently have an armed window.
    pub fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .len()
    }
}

/// Park hook the reaper awaits: wakes on a new arm or batcher drop.
async fn park(this: &std::sync::Weak<PendingBatcher>) {
    match this.upgrade() {
        Some(batch) => batch.armed.notified().await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A tiny real-window test: schedule, then wait for the flush.
    #[tokio::test]
    async fn debounce_coalesces_with_real_clock() {
        let b = PendingBatcher::new(Duration::from_millis(30));
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        b.schedule("s", move || {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        // A burst of further schedules on the same key must not add runs.
        for _ in 0..4 {
            let c3 = calls.clone();
            b.schedule("s", move || {
                c3.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(b.pending_count(), 1, "burst must keep a single entry");
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a debounced burst flushes exactly once"
        );
        assert_eq!(b.pending_count(), 0, "entry cleaned up after flush");
    }

    #[tokio::test]
    async fn multi_session_independent() {
        let b = PendingBatcher::new(Duration::from_millis(20));
        let a = Arc::new(AtomicUsize::new(0));
        let c = Arc::new(AtomicUsize::new(0));
        let a2 = a.clone();
        b.schedule("a", move || {
            a2.fetch_add(1, Ordering::SeqCst);
        });
        let c2 = c.clone();
        b.schedule("c", move || {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(b.pending_count(), 2);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(a.load(Ordering::SeqCst), 1);
        assert_eq!(c.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_millis(100), b.flush_all())
            .await
            .expect("completed due callbacks must not strand inflight accounting");
    }

    #[tokio::test]
    async fn flush_all_drains_pending_and_becomes_terminal() {
        let b = PendingBatcher::new(Duration::from_secs(60));
        let a = Arc::new(AtomicUsize::new(0));
        let c = Arc::new(AtomicUsize::new(0));
        let a2 = a.clone();
        b.schedule("a", move || {
            a2.fetch_add(1, Ordering::SeqCst);
        });
        let c2 = c.clone();
        b.schedule("c", move || {
            c2.fetch_add(1, Ordering::SeqCst);
        });

        b.flush_all().await;
        assert_eq!(a.load(Ordering::SeqCst), 1, "flush_all drains pending");
        assert_eq!(c.load(Ordering::SeqCst), 1);
        assert_eq!(b.pending_count(), 0);

        // After FlushAll the batcher is terminal: schedule runs inline.
        let ran = Arc::new(AtomicUsize::new(0));
        let r2 = ran.clone();
        b.schedule("d", move || {
            r2.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(ran.load(Ordering::SeqCst), 1, "post-drain schedule inlines");
    }

    #[test]
    fn defaults_window_when_non_positive() {
        assert_eq!(
            PendingBatcher::new(Duration::ZERO).window(),
            DEFAULT_CHAT_RUN_BATCH_WINDOW
        );
        let w = Duration::from_millis(500);
        assert_eq!(PendingBatcher::new(w).window(), w);
    }
}
