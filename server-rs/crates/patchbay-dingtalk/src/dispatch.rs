//! Port of `dispatch.go`: decouples inbound processing from the Stream read
//! loop. Frames are ACKed immediately and jobs run on per-conversation serial
//! queues, so a slow media download can neither starve ping/system frames nor
//! reorder a conversation's Agent event history. Cross-conversation jobs run in
//! parallel, bounded by a global semaphore per installation. The
//! per-conversation queue is bounded only as a memory backstop, set high enough
//! that a real human burst never reaches it; the engine's dedup makes any
//! duplicate delivery harmless.
//!
//! Jobs run on their own context with their own deadline, deliberately detached
//! from the socket's run context: a gateway redial must not cancel an in-flight
//! append.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use patchbay_channel::InboundMessage;

/// Bounds the synchronous ingest path (remote media resolution is detached by
/// the shared Router).
pub const DISPATCH_JOB_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_DISPATCH_WORKERS: usize = 8;
/// Bounds one conversation's backlog purely as a memory-safety backstop. It is
/// set far above any realistic human burst so the overflow drop is effectively
/// unreachable in practice. Overflow drops the newest message with a warn log;
/// the caller (the socket read loop) must never block.
const MAX_DISPATCH_QUEUE_DEPTH: usize = 256;
/// A per-conversation limit alone still permits one waiting drain task per
/// distinct conversation. Bound all accepted-but-not-finished messages for one
/// installation so both queued payloads and semaphore waiters have a
/// deterministic ceiling.
const MAX_DISPATCH_PENDING: usize = 2048;

pub type DispatchJobFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
pub type DispatchHandle =
    Arc<dyn Fn(CancellationToken, InboundMessage) -> DispatchJobFuture + Send + Sync>;

struct Inner {
    queues: HashMap<String, VecDeque<InboundMessage>>,
    active: HashSet<String>,
    closed: bool,
    pending: usize,
}

/// State shared between the dispatcher handle and its spawned drain tasks.
struct Shared {
    inner: Mutex<Inner>,
    workers_tx: tokio::sync::watch::Sender<usize>,
    workers_rx: tokio::sync::watch::Receiver<usize>,
    /// Fires once every worker finished after close (Go's `done` channel).
    drained: CancellationToken,
}

/// The per-installation job dispatcher (Go `dispatcher`).
pub struct Dispatcher {
    handle: DispatchHandle,
    sem: Arc<tokio::sync::Semaphore>,
    ctx: CancellationToken,
    shared: Arc<Shared>,
}

impl Dispatcher {
    pub fn new(handle: DispatchHandle) -> Self {
        let (workers_tx, workers_rx) = tokio::sync::watch::channel(0usize);
        Self {
            handle,
            sem: Arc::new(tokio::sync::Semaphore::new(MAX_DISPATCH_WORKERS)),
            ctx: CancellationToken::new(),
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    queues: HashMap::new(),
                    active: HashSet::new(),
                    closed: false,
                    pending: 0,
                }),
                workers_tx,
                workers_rx,
                drained: CancellationToken::new(),
            }),
        }
    }

    /// Appends msg to its conversation's queue and starts a drain worker for
    /// the conversation when none is running. Never blocks the caller.
    pub fn enqueue(&self, conv_id: &str, msg: InboundMessage) {
        let start = {
            let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.closed {
                drop(inner);
                tracing::debug!(
                    conversation_id = conv_id,
                    msg_id = %msg.message_id,
                    "dingtalk dispatch: dispatcher closed, dropping message"
                );
                return;
            }
            if inner.pending >= MAX_DISPATCH_PENDING {
                drop(inner);
                tracing::warn!(
                    conversation_id = conv_id,
                    msg_id = %msg.message_id,
                    "dingtalk dispatch: installation queue full, dropping message"
                );
                return;
            }
            if inner
                .queues
                .get(conv_id)
                .is_some_and(|q| q.len() >= MAX_DISPATCH_QUEUE_DEPTH)
            {
                drop(inner);
                tracing::warn!(
                    conversation_id = conv_id,
                    msg_id = %msg.message_id,
                    "dingtalk dispatch: conversation queue full, dropping message"
                );
                return;
            }
            inner
                .queues
                .entry(conv_id.to_string())
                .or_default()
                .push_back(msg);
            inner.pending += 1;
            let start = !inner.active.contains(conv_id);
            if start {
                inner.active.insert(conv_id.to_string());
                // Counted inside the critical section (Go: d.workers.Add(1)
                // before Unlock) so a concurrent start_close can never observe
                // a zero counter while a spawn is still in flight.
                self.shared.workers_tx.send_modify(|c| *c += 1);
            }
            start
        };
        if start {
            tokio::spawn(drain(
                self.handle.clone(),
                self.sem.clone(),
                self.ctx.clone(),
                self.shared.clone(),
                conv_id.to_string(),
            ));
        }
    }

    /// Stops accepting new jobs and waits for already-ACKed messages to
    /// finish. If ctx expires, it cancels every active job and leaves the
    /// worker joins to complete asynchronously; callers are never held past
    /// their shutdown budget. It is idempotent.
    pub async fn drain_and_close(&self, ctx: CancellationToken) -> bool {
        self.start_close();
        self.wait_closed(ctx).await
    }

    /// Atomically stops new enqueues and starts the asynchronous worker join.
    /// Splitting this from wait_closed lets the owning dispatch slot publish
    /// the closing state before a reconnect decides whether the queue can be
    /// reused.
    pub fn start_close(&self) {
        let spawn_join = {
            let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if inner.closed {
                None
            } else {
                inner.closed = true;
                Some((self.shared.clone(), self.ctx.clone()))
            }
        };
        if let Some((shared, ctx)) = spawn_join {
            tokio::spawn(async move {
                let mut rx = shared.workers_rx.clone();
                while *rx.borrow_and_update() > 0 {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                // Workers are gone; cancel the job context so queued-but-
                // unstarted work is discarded by any straggler drain.
                ctx.cancel();
                shared.drained.cancel();
            });
        }
    }

    pub async fn wait_closed(&self, ctx: CancellationToken) -> bool {
        tokio::select! {
            _ = self.shared.drained.cancelled() => true,
            _ = ctx.cancelled() => {
                self.ctx.cancel();
                // Non-blocking re-check mirrors Go's default branch.
                tokio::select! {
                    _ = self.shared.drained.cancelled() => true,
                    _ = std::future::ready(()) => false,
                }
            }
        }
    }

    /// Cancels active jobs and queued work after an owner's graceful drain
    /// budget expires. Workers observe this token in both semaphore waits and
    /// in-flight job polling, then publish `drained` through the close joiner.
    pub fn cancel(&self) {
        self.ctx.cancel();
    }

    pub fn is_closed(&self) -> bool {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed
    }
}

/// Runs the conversation's jobs strictly in order and exits when the queue is
/// empty (a later enqueue starts a fresh worker). The semaphore bounds
/// concurrently running jobs across all conversations; waiting on it keeps this
/// conversation's order intact.
async fn drain(
    handle: DispatchHandle,
    sem: Arc<tokio::sync::Semaphore>,
    ctx: CancellationToken,
    shared: Arc<Shared>,
    conv_id: String,
) {
    loop {
        let msg = {
            let mut inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if ctx.is_cancelled() {
                discard_queue(&mut inner, &shared, &conv_id, 0);
                return;
            }
            match inner.queues.get_mut(&conv_id) {
                Some(q) if !q.is_empty() => q.pop_front().expect("queue checked non-empty"),
                _ => {
                    inner.queues.remove(&conv_id);
                    inner.active.remove(&conv_id);
                    drop(inner);
                    finish_worker(&shared);
                    return;
                }
            }
        };

        let permit = tokio::select! {
            p = sem.acquire() => p.expect("semaphore never closed"),
            _ = ctx.cancelled() => {
                let mut inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                discard_queue(&mut inner, &shared, &conv_id, 1);
                return;
            }
        };

        run_job(&handle, &ctx, msg).await;
        drop(permit);
        let mut inner = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.pending = inner.pending.saturating_sub(1);
    }
}

/// Drops the conversation's remaining backlog from the pending count and clears
/// its active marker. `extra` counts messages already popped but not yet
/// processed (the one held when cancellation hit).
fn discard_queue(inner: &mut Inner, shared: &Shared, conv_id: &str, extra: usize) {
    let dropped = inner.queues.remove(conv_id).map_or(0, |q| q.len());
    inner.pending = inner.pending.saturating_sub(dropped + extra);
    inner.active.remove(conv_id);
    finish_worker(shared);
}

fn finish_worker(shared: &Shared) {
    shared.workers_tx.send_modify(|c| *c = c.saturating_sub(1));
}

/// Runs one job on a child context with its own deadline, deliberately detached
/// from the socket's run context. A graceful dispatcher close leaves accepted
/// work alone; a lifecycle cancellation or job deadline cancels and drops the
/// future so no worker can outlive the dispatcher's bounded shutdown.
async fn run_job(handle: &DispatchHandle, ctx: &CancellationToken, msg: InboundMessage) {
    let job_ctx = ctx.child_token();
    let cancel = job_ctx.clone();
    let fut = handle(job_ctx, msg);
    tokio::pin!(fut);
    tokio::select! {
        biased;
        _ = ctx.cancelled() => cancel.cancel(),
        _ = &mut fut => {}
        _ = tokio::time::sleep(DISPATCH_JOB_TIMEOUT) => {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counting_handle(counter: Arc<AtomicUsize>, delay: Duration) -> DispatchHandle {
        Arc::new(move |_ctx, msg| {
            let counter = counter.clone();
            let delay = delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                counter.fetch_add(msg.text.parse::<usize>().unwrap_or(0), Ordering::SeqCst);
            })
        })
    }

    #[tokio::test]
    async fn same_conversation_runs_in_order() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let order2 = order.clone();
        let handle: DispatchHandle = Arc::new(move |_ctx, msg| {
            let order = order2.clone();
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                order.lock().unwrap().push(msg.text.clone());
            })
        });
        let d = Dispatcher::new(handle);
        for i in 0..10 {
            d.enqueue(
                "conv",
                InboundMessage {
                    text: i.to_string(),
                    ..Default::default()
                },
            );
        }
        assert!(d.drain_and_close(CancellationToken::new()).await);
        assert_eq!(
            *order.lock().unwrap(),
            (0..10).map(|i| i.to_string()).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn different_conversations_run_in_parallel() {
        let started = Arc::new(tokio::sync::Barrier::new(4));
        let done = Arc::new(AtomicUsize::new(0));
        let handle: DispatchHandle = {
            let started = started.clone();
            let done = done.clone();
            Arc::new(move |_ctx, msg| {
                let started = started.clone();
                let done = done.clone();
                Box::pin(async move {
                    // All four conversations must enter the handler before
                    // any one can finish, so this checks concurrency without
                    // relying on a wall-clock threshold under CI load.
                    started.wait().await;
                    done.fetch_add(msg.text.parse::<usize>().unwrap_or(0), Ordering::SeqCst);
                })
            })
        };
        let d = Dispatcher::new(handle);
        for c in ["a", "b", "c", "d"] {
            d.enqueue(
                c,
                InboundMessage {
                    text: "1".into(),
                    ..Default::default()
                },
            );
        }
        let drained = tokio::time::timeout(
            Duration::from_secs(5),
            d.drain_and_close(CancellationToken::new()),
        )
        .await
        .expect("different conversations should reach the handler concurrently");
        assert!(drained);
        assert_eq!(done.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn closed_dispatcher_drops_new_messages() {
        let counter = Arc::new(AtomicUsize::new(0));
        let d = Dispatcher::new(counting_handle(counter.clone(), Duration::from_millis(1)));
        d.enqueue(
            "c",
            InboundMessage {
                text: "1".into(),
                ..Default::default()
            },
        );
        assert!(d.drain_and_close(CancellationToken::new()).await);
        assert!(d.is_closed());
        d.enqueue(
            "c",
            InboundMessage {
                text: "2".into(),
                ..Default::default()
            },
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn wait_closed_honors_caller_deadline() {
        let blocker: DispatchHandle = Arc::new(|_ctx, _msg| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
            })
        });
        let d = Dispatcher::new(blocker);
        d.enqueue("c", InboundMessage::default());
        d.start_close();
        let deadline = CancellationToken::new();
        let stop = deadline.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            stop.cancel();
        });
        let ok = d.wait_closed(deadline).await;
        assert!(!ok);
        assert!(tokio::time::timeout(
            Duration::from_secs(1),
            d.wait_closed(CancellationToken::new())
        )
        .await
        .expect("cancelled worker should publish drained"));
    }
}
