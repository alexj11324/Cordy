//! Channel-media intent ledger reconciler.
//!
//! Settles ledger rows written before each upload and cleared inside the
//! attachment-bind transaction. Whatever survives — upload errors, resolve
//! deadlines, bind failures, ambiguous commits, crashes — is claimed here
//! ('pending' → 'deleting' under a lease), re-checked for a durable
//! attachment reference AFTER the claim (race-free: a bind can never attach a
//! claimed key), then either kept or deleted from object storage.
//!
//! Fencing model:
//!   - bind vs. delete is fenced by STATE — once claimed 'deleting', neither
//!     an upload nor BindMediaRefs can resurrect the key;
//!   - an abandoned PUT that materializes after its DELETE is fenced by the
//!     TOMBSTONE schedule: the row survives and the object is re-deleted on a
//!     widening schedule; every pass re-checks references first.
//!
//! All deadlines are DURATIONS handed to SQL — Postgres derives cutoffs from
//! its own clock, so drifted replicas cannot settle early or inherit expired
//! leases.

use std::sync::Arc;
use std::time::Duration;

use sqlx::postgres::types::PgInterval;
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_db::models::ChannelMediaPendingObject;
use patchbay_db::queries::channel::{
    channel_media_object_is_referenced, claim_next_channel_media_pending_object_for_reconcile,
    count_channel_media_pending_objects, delete_channel_media_pending_object,
    release_channel_media_pending_object, tombstone_channel_media_pending_object,
};

/// How long a 'pending' row must sit before it counts as abandoned.
/// Exported parity with Go's invariant-test constant.
pub const CHANNEL_MEDIA_RECONCILE_SETTLE_DELAY: Duration = Duration::from_secs(15 * 60);

/// Paces the reconciler loop.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounds how long a claimed row stays owned before another replica may
/// reclaim it (crash recovery).
const RECONCILE_LEASE: Duration = Duration::from_secs(2 * 60);

/// Caps how many settle operations one sweep performs. Rows are claimed one
/// at a time and settled sequentially, so this counts settles, not distinct
/// rows — it just keeps a sweep from running away.
const SWEEP_LIMIT: usize = 50;

/// Backoff for failed object-storage deletes: base << (attempt-1), capped.
const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_CAP: Duration = Duration::from_secs(60 * 60);

/// Bounds ONE object-storage DELETE so a black-holed connection cannot wedge
/// the sequential sweep (single-replica deployments have no other worker).
/// Kept well under the lease so a timed-out delete releases with backoff
/// before any replica could reclaim the row.
const DELETE_TIMEOUT: Duration = Duration::from_secs(30);

const TOMBSTONED_STATE: &str = "tombstoned";

/// Widening re-delete schedule for a tombstoned row, indexed by
/// tombstone_pass: an abandoned PUT can still materialize the object after
/// the delete, so each entry triggers another idempotent delete and the row
/// drops only after the last one (~31h total coverage).
const TOMBSTONE_REDELETE: [Duration; 4] = [
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(6 * 60 * 60),
    Duration::from_secs(24 * 60 * 60),
];

/// The single storage capability the reconciler needs: Delete with the error
/// surfaced so failures go to backoff instead of being assumed successful.
#[async_trait::async_trait]
pub trait MediaObjectDeleter: Send + Sync {
    async fn delete_object(&self, key: &str) -> anyhow::Result<()>;
}

/// Carries a duration to SQL so Postgres computes the deadline itself: every
/// settle/lease/retry decision compares against the DATABASE clock, so a
/// replica whose own clock drifted can neither settle a row early nor hand
/// out a born-expired lease.
fn pg_interval(d: Duration) -> PgInterval {
    PgInterval {
        microseconds: d.as_micros() as i64,
        days: 0,
        months: 0,
    }
}

/// Reconciler worker. Assembly builds it only when a storage backend exists;
/// `storage` stays an Option so a mis-wired instance skips its sweep instead
/// of stranding rows.
pub struct ChannelMediaReconciler {
    pub pool: PgPool,
    pub storage: Option<Arc<dyn MediaObjectDeleter>>,
    pub metrics: Option<Arc<patchbay_metrics::channel_media::ChannelMediaReconcilerMetrics>>,
    /// Overridable for deterministic tests; None falls back to
    /// [`DELETE_TIMEOUT`].
    pub delete_timeout: Option<Duration>,
    config: ChannelMediaReconcilerConfig,
}

/// Process-lifecycle settings. The database remains the authoritative clock
/// for settle, lease, retry, and tombstone deadlines; only loop pacing and
/// bounded shutdown are injected here.
#[derive(Debug, Clone, Copy)]
pub struct ChannelMediaReconcilerConfig {
    pub sweep_interval: Duration,
    pub shutdown_timeout: Duration,
}

impl Default for ChannelMediaReconcilerConfig {
    fn default() -> Self {
        Self {
            sweep_interval: SWEEP_INTERVAL,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

impl ChannelMediaReconciler {
    pub fn new(
        pool: PgPool,
        storage: Arc<dyn MediaObjectDeleter>,
        metrics: Option<Arc<patchbay_metrics::channel_media::ChannelMediaReconcilerMetrics>>,
    ) -> Self {
        Self {
            pool,
            storage: Some(storage),
            metrics,
            delete_timeout: None,
            config: ChannelMediaReconcilerConfig::default(),
        }
    }

    pub fn with_config(mut self, config: ChannelMediaReconcilerConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !config.sweep_interval.is_zero(),
            "channel media reconciler sweep interval must be positive"
        );
        anyhow::ensure!(
            !config.shutdown_timeout.is_zero(),
            "channel media reconciler shutdown timeout must be positive"
        );
        self.config = config;
        Ok(self)
    }

    /// Starts the independent reconciler task and returns its owned lifecycle.
    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> ChannelMediaReconcilerRuntime {
        let shutdown_timeout = self.config.shutdown_timeout;
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        ChannelMediaReconcilerRuntime {
            cancel,
            task: Some(task),
            shutdown_timeout,
        }
    }

    /// Loops [`run_once`](Self::run_once) until cancelled. Started as its own
    /// task from server assembly; deliberately not coupled to any other
    /// sweeper's cadence. Mirrors Go's time.NewTicker: the first sweep fires
    /// only after a full interval.
    pub async fn run(&self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(self.config.sweep_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume tokio's immediate first tick — Go's NewTicker fires only
        // after the full interval.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => self.run_once(&cancel).await,
            }
        }
    }

    /// Settles due ledger rows one at a time: claim, settle, claim next, up
    /// to the per-sweep limit. Each claim is taken immediately before that
    /// row's work so no row holds a lease it waits behind — its attempt
    /// counter and backoff describe deletes actually tried. All errors are
    /// per-row and non-fatal: an unsettled row backs off and retries on a
    /// later sweep (or by another replica after lease expiry).
    pub async fn run_once(&self, cancel: &CancellationToken) {
        let Some(storage) = &self.storage else {
            // A panic here would take down the whole process from a bare
            // task — keep the invariant local. Claiming without a deleter
            // would strand rows in 'deleting' until lease expiry.
            tracing::error!("channel media reconciler: no storage backend; skipping sweep");
            return;
        };
        for _ in 0..SWEEP_LIMIT {
            let lease_token = patchbay_db::dbid::new_v7();
            let row = claim_next_channel_media_pending_object_for_reconcile(
                &self.pool,
                lease_token,
                pg_interval(RECONCILE_LEASE),
                pg_interval(CHANNEL_MEDIA_RECONCILE_SETTLE_DELAY),
            )
            .await;
            let row = match row {
                Ok(Some(row)) => row,
                // No due row: the normal quiet path.
                Ok(None) => break,
                Err(err) => {
                    // Shutdown cancels the sweep mid-loop; that is the normal
                    // way a sweep ends, not a failure worth waking anyone for.
                    if cancel.is_cancelled() {
                        return;
                    }
                    tracing::warn!(error = %err, "channel media reconciler: claim failed");
                    break;
                }
            };
            self.settle(cancel, storage.clone(), row, lease_token).await;
        }
        if let Some(metrics) = &self.metrics {
            if let Ok(Some(counts)) = count_channel_media_pending_objects(&self.pool).await {
                metrics.backlog.set(counts.pending_objects as f64);
                metrics.tombstones.set(counts.tombstoned_objects as f64);
            }
        }
    }

    /// Runs the row the caller just claimed. The lease only has to cover this
    /// one row (delete timeout << lease), so no heartbeat is needed: every
    /// write below is lease-token guarded, so a row reclaimed after an expiry
    /// ignores the old owner's writes rather than being corrupted by them.
    async fn settle(
        &self,
        cancel: &CancellationToken,
        storage: Arc<dyn MediaObjectDeleter>,
        row: ChannelMediaPendingObject,
        lease_token: Uuid,
    ) {
        // The reference check runs AFTER the claim flipped the row to
        // 'deleting': from that point a bind cannot attach this key, so a
        // negative answer is terminal, not a snapshot race. It runs on
        // tombstone passes too — the object a tombstone re-deletes and the
        // object an attachment reads are the same key, so skipping the check
        // would manufacture the dangling attachment this ledger exists to
        // prevent.
        let referenced = channel_media_object_is_referenced(
            &self.pool,
            row.chat_message_id,
            row.workspace_id,
            &row.storage_url,
        )
        .await;
        let referenced = match referenced {
            Ok(v) => v.unwrap_or(false),
            Err(err) => {
                self.release(cancel, &row, lease_token, err).await;
                return;
            }
        };
        if referenced {
            if row.state == TOMBSTONED_STATE {
                // Unreachable by design — object keys are per (chat message,
                // resource), so a re-ingest gets its own key and a bind can
                // never attach a key that left 'pending'. Reaching it means an
                // invariant broke; the safe answer is keep the object and
                // raise an anomaly, never delete a live one.
                tracing::error!(
                    storage_key = %row.storage_key,
                    workspace_id = %row.workspace_id,
                    chat_message_id = %row.chat_message_id,
                    tombstone_pass = row.tombstone_pass,
                    "channel media reconciler: tombstoned object is referenced by an attachment; keeping it"
                );
                if let Some(metrics) = &self.metrics {
                    metrics.tombstone_referenced.inc();
                }
            }
            // The bind landed (its transaction lost the pending-row race to
            // an earlier claim, or a redelivered intent outlived the bind).
            // Keep the object, clear the row.
            if self.clear_row(cancel, &row, lease_token).await {
                tracing::info!(
                    storage_key = %row.storage_key,
                    workspace_id = %row.workspace_id,
                    chat_message_id = %row.chat_message_id,
                    "channel media reconciler: kept referenced object"
                );
                if let Some(metrics) = &self.metrics {
                    metrics.rows_referenced.inc();
                }
            }
            return;
        }
        self.settle_deleted_object(cancel, storage, row, lease_token)
            .await;
    }

    /// Deletes the object and then either tombstones the row for a later
    /// re-delete pass or clears it once the schedule is exhausted. Shared by
    /// the first (unreferenced) settle and every tombstone revisit.
    async fn settle_deleted_object(
        &self,
        cancel: &CancellationToken,
        storage: Arc<dyn MediaObjectDeleter>,
        row: ChannelMediaPendingObject,
        lease_token: Uuid,
    ) {
        // The delete runs OUTSIDE any transaction (no DB lock held across
        // storage I/O), gated by the lease and bounded by its own timeout so
        // one stalled connection cannot wedge the sequential sweep.
        let del_timeout = self.delete_timeout.unwrap_or(DELETE_TIMEOUT);
        let deleted =
            tokio::time::timeout(del_timeout, storage.delete_object(&row.storage_key)).await;
        match deleted {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                if let Some(metrics) = &self.metrics {
                    metrics.delete_failures.inc();
                }
                self.release(cancel, &row, lease_token, err).await;
                return;
            }
            // A black-holed delete times out into the same backoff path; the
            // surfaced cause keeps the ambiguity visible in last_error.
            Err(_) => {
                if let Some(metrics) = &self.metrics {
                    metrics.delete_failures.inc();
                }
                self.release(cancel, &row, lease_token, "object-storage delete timed out")
                    .await;
                return;
            }
        }
        // The delete succeeded, but an abandoned PUT may still materialize
        // the object afterwards. Tombstone and re-delete on the widening
        // schedule; drop the row only once the schedule is exhausted.
        let (next, idx) = match next_tombstone_pass(&row) {
            Some(pair) => pair,
            None => {
                if self.clear_row(cancel, &row, lease_token).await {
                    tracing::info!(
                        storage_key = %row.storage_key,
                        workspace_id = %row.workspace_id,
                        chat_message_id = %row.chat_message_id,
                        attempt = row.attempt,
                        "channel media reconciler: tombstone schedule exhausted; ledger row cleared"
                    );
                }
                return;
            }
        };
        if !self
            .tombstone_row(cancel, &row, lease_token, next, idx)
            .await
        {
            return;
        }
        // Re-delete passes are idempotent: usually nothing was there.
        let msg = if row.state == TOMBSTONED_STATE {
            "channel media reconciler: tombstone re-delete pass done"
        } else {
            "channel media reconciler: deleted unreferenced object; tombstoned for re-delete"
        };
        tracing::info!(
            storage_key = %row.storage_key,
            workspace_id = %row.workspace_id,
            chat_message_id = %row.chat_message_id,
            attempt = row.attempt,
            tombstone_pass = idx,
            next_redelete_in_secs = next.as_secs(),
            "{msg}"
        );
        if let Some(metrics) = &self.metrics {
            // Count the object once, on the delete that FIRST removed it.
            if row.state != TOMBSTONED_STATE {
                metrics.objects_deleted.inc();
            }
        }
    }

    /// Releases a failed settle: keeps the row in 'deleting' (a bind must
    /// still never attach it), drops the lease, backs off the next attempt.
    async fn release(
        &self,
        cancel: &CancellationToken,
        row: &ChannelMediaPendingObject,
        lease_token: Uuid,
        cause: impl std::fmt::Display,
    ) {
        if cancel.is_cancelled() {
            // Shutdown cancelled the settle itself; the backoff write would
            // fail on the same cancelled context. Lease expiry reclaims the
            // row like any other interrupted worker.
            return;
        }
        let backoff = reconcile_backoff(row.attempt);
        let cause_text = cause.to_string();
        tracing::warn!(
            storage_key = %row.storage_key,
            workspace_id = %row.workspace_id,
            attempt = row.attempt,
            backoff_secs = backoff.as_secs(),
            error = %cause_text,
            "channel media reconciler: settle failed; backing off"
        );
        if let Err(err) = release_channel_media_pending_object(
            &self.pool,
            pg_interval(backoff),
            Some(cause_text.as_str()),
            &row.storage_key,
            row.workspace_id,
            lease_token,
        )
        .await
        {
            // Lease expiry reclaims the row regardless.
            tracing::warn!(
                storage_key = %row.storage_key,
                error = %err,
                "channel media reconciler: release failed"
            );
        }
    }

    /// Deletes the ledger row under the lease token; false when nothing was
    /// cleared or shutdown intervened.
    async fn clear_row(
        &self,
        cancel: &CancellationToken,
        row: &ChannelMediaPendingObject,
        lease_token: Uuid,
    ) -> bool {
        if cancel.is_cancelled() {
            // Shutdown mid-settle leaves the row to its lease expiry rather
            // than logging a write that never had a chance to land.
            return false;
        }
        match delete_channel_media_pending_object(
            &self.pool,
            &row.storage_key,
            row.workspace_id,
            lease_token,
        )
        .await
        {
            Ok(n) => n > 0,
            Err(err) => {
                tracing::warn!(
                    storage_key = %row.storage_key,
                    error = %err,
                    "channel media reconciler: clear row failed"
                );
                false
            }
        }
    }

    async fn tombstone_row(
        &self,
        cancel: &CancellationToken,
        row: &ChannelMediaPendingObject,
        lease_token: Uuid,
        next: Duration,
        idx: usize,
    ) -> bool {
        if cancel.is_cancelled() {
            // Same as release: a cancelled context cannot carry the write, and
            // shutdown is not a failure. The next pass re-deletes idempotently.
            return false;
        }
        let n = tombstone_channel_media_pending_object(
            &self.pool,
            pg_interval(next),
            idx as i32,
            &row.storage_key,
            row.workspace_id,
            lease_token,
        )
        .await;
        match n {
            Ok(n) => n > 0,
            Err(err) => {
                tracing::warn!(
                    storage_key = %row.storage_key,
                    error = %err,
                    "channel media reconciler: tombstone failed"
                );
                false
            }
        }
    }
}

pub struct ChannelMediaReconcilerRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    shutdown_timeout: Duration,
}

impl ChannelMediaReconcilerRuntime {
    pub async fn shutdown(mut self) -> ChannelMediaShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return ChannelMediaShutdownOutcome::Panicked;
        };
        match tokio::time::timeout(self.shutdown_timeout, &mut task).await {
            Ok(Ok(())) => ChannelMediaShutdownOutcome::Stopped,
            Ok(Err(_)) => ChannelMediaShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                ChannelMediaShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for ChannelMediaReconcilerRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMediaShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

/// Returns the delay before this row's next re-delete pass and the schedule
/// position to record, or None when the schedule is exhausted and the row may
/// drop. A first-time deletion (state 'deleting') starts at index 0; a
/// tombstone advances from its recorded tombstone_pass so a failed re-delete
/// in between (which only writes last_error/attempt) cannot restart the walk.
fn next_tombstone_pass(row: &ChannelMediaPendingObject) -> Option<(Duration, usize)> {
    let idx = if row.state == TOMBSTONED_STATE {
        row.tombstone_pass.max(0) as usize + 1
    } else {
        0
    };
    TOMBSTONE_REDELETE.get(idx).map(|d| (*d, idx))
}

/// base << min(attempt-1, 10), clamped to the cap. Saturating math guards a
/// zero/negative attempt against shift underflow.
fn reconcile_backoff(attempt: i32) -> Duration {
    let shift = attempt.saturating_sub(1).clamp(0, 10) as u32;
    let backoff = BACKOFF_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(BACKOFF_CAP);
    if backoff > BACKOFF_CAP || backoff.is_zero() {
        BACKOFF_CAP
    } else {
        backoff
    }
}
