//! The health signals this adapter emits — port of `metrics.go`.
//!
//! Every failure in the connection path degrades quietly by design. A dial
//! that fails and a handshake the server refuses hand the connection back to
//! the Supervisor for backoff and retry; a full ingest queue parks the read
//! loop instead, and the socket stops being drained until the worker catches
//! up. Quiet is the right behaviour for the person in front of the chat and
//! the wrong behaviour for the operator behind it — nothing on a dashboard
//! changes when a bot has been unable to connect for an hour.
//!
//! The counters here are chosen for what somebody would page on rather than
//! for completeness: the connection is not coming up, and if so whether that
//! needs a person or just time; and the read loop is being made to wait by an
//! ingest worker that cannot keep up.
//!
//! No installation id anywhere. It is an unbounded identifier and the metrics
//! package rejects that class of label outright. What attribution exists is
//! in the structured logs, and it is not uniform: the two connection counters
//! have it, because the failure they count is also returned to the Supervisor,
//! which logs it with installation_id. The two inbound counters have nothing
//! beside them — a queue that blocks writes no log line at all, so the
//! counter tells an operator that some bot is behind and not which one.

/// The sink this adapter reports to. Every method must tolerate being called
/// concurrently, and none of them may block: they run on the read loop.
///
/// Port note: Go's interface becomes an async-trait-free object-safe trait;
/// the production implementation lives in `cordy-metrics::WecomMetrics` and
/// is wired in at boot, exactly as Go wires `internal/metrics`' sink.
pub trait Metrics: Send + Sync {
    /// A dial, a handshake write or a handshake read that did not complete,
    /// or a handshake the server answered with a code that
    /// classify_subscribe_ack could not verify (a throttle, a platform-side
    /// failure). Excludes an outright credential rejection, which has its own
    /// counter: everything counted here recovers on its own, that one needs
    /// an admin.
    fn record_connect_failure(&self);

    /// aibot_subscribe was refused on the credentials, as
    /// classify_subscribe_ack judges it (ErrCredentialsRejected:
    /// 40001 / 40013). Deliberately not every non-zero errcode — the codes
    /// that only mean "could not verify" go to
    /// [`Metrics::record_connect_failure`], because paging an operator to
    /// rotate a good secret costs a second outage. The bot will not connect
    /// until somebody fixes the credentials, so a sustained rate here is an
    /// alert and not a blip.
    fn record_auth_failure(&self);

    /// One inbound callback handed to the worker. The baseline every other
    /// inbound number is read against.
    fn record_callback_queued(&self);

    /// The worker queue was full and the read loop had to wait.
    /// Backpressure, deliberately: it is how a slow ingest stops rather than
    /// a message being dropped. A rising rate says the engine is not keeping
    /// up with one bot's traffic, and past a point WeCom stops seeing the
    /// socket drained and replaces the connection.
    fn record_callback_queue_blocked(&self);
}

/// What every constructor falls back to. A missing sink must never surface as
/// a panic on the read loop.
#[derive(Debug, Default, Clone, Copy)]
pub struct NopMetrics;

impl Metrics for NopMetrics {
    fn record_connect_failure(&self) {}
    fn record_auth_failure(&self) {}
    fn record_callback_queued(&self) {}
    fn record_callback_queue_blocked(&self) {}
}

/// Turns an unset sink into one that is safe to call.
///
/// Port note: Go's `orNopMetrics(m Metrics) Metrics`; Rust callers hold
/// `Arc<dyn Metrics>` so the substitution happens where the field is built.
pub fn or_nop_metrics(m: Option<std::sync::Arc<dyn Metrics>>) -> std::sync::Arc<dyn Metrics> {
    m.unwrap_or(std::sync::Arc::new(NopMetrics))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting {
        connects: AtomicUsize,
        auths: AtomicUsize,
        queued: AtomicUsize,
        blocked: AtomicUsize,
    }

    impl Counting {
        fn new() -> Self {
            Self {
                connects: AtomicUsize::new(0),
                auths: AtomicUsize::new(0),
                queued: AtomicUsize::new(0),
                blocked: AtomicUsize::new(0),
            }
        }
    }

    impl Metrics for Counting {
        fn record_connect_failure(&self) {
            self.connects.fetch_add(1, Ordering::SeqCst);
        }
        fn record_auth_failure(&self) {
            self.auths.fetch_add(1, Ordering::SeqCst);
        }
        fn record_callback_queued(&self) {
            self.queued.fetch_add(1, Ordering::SeqCst);
        }
        fn record_callback_queue_blocked(&self) {
            self.blocked.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn nop_sink_is_safe_to_call() {
        let m = or_nop_metrics(None);
        m.record_connect_failure();
        m.record_auth_failure();
        m.record_callback_queued();
        m.record_callback_queue_blocked();
    }

    #[test]
    fn counters_track_each_verdict_separately() {
        let c = std::sync::Arc::new(Counting::new());
        let m: std::sync::Arc<dyn Metrics> = c.clone();
        m.record_connect_failure();
        m.record_connect_failure();
        m.record_auth_failure();
        m.record_callback_queued();
        m.record_callback_queue_blocked();
        assert_eq!(c.connects.load(Ordering::SeqCst), 2);
        assert_eq!(c.auths.load(Ordering::SeqCst), 1);
        assert_eq!(c.queued.load(Ordering::SeqCst), 1);
        assert_eq!(c.blocked.load(Ordering::SeqCst), 1);
    }
}
