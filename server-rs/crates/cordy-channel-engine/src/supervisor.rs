//! The per-installation connection supervisor.
//!
//! (978 lines). It enumerates active installations across ALL channel
//! types (no hard-coded platform), fences each behind the WS lease CAS so
//! at most one replica connects per installation, builds the platform
//! Channel via the `cordy_channel::Registry`, drives its
//! connect/disconnect lifecycle with exponential backoff + jitter, and
//! restarts a connection whose credentials rotated.
//!
//! Port note: Go's goroutines capture `&Supervisor` and rely on the GC;
//! Rust tasks hold `Arc<Self>` clones instead (no unsafe self-references).
//! `context.Context` becomes [`CancellationToken`]; WaitGroup becomes
//! per-entry oneshot completion channels.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use cordy_channel::{BuiltChannel, Config as ChannelConfig, LeaseGeneration, Registry};

use crate::ids::new_node_id;
use crate::lease::{AcquireLeaseParams, LeaseError, LeaseStore, ReleaseLeaseParams};

type SharedAbortHandle = Arc<Mutex<Option<tokio::task::AbortHandle>>>;

/// One active installation row the supervisor may lease and drive.
/// Mirrors Go `engine.Installation`.
#[derive(Debug, Clone)]
pub struct Installation {
    /// channel_installation.id.
    pub id: uuid::Uuid,
    /// Platform discriminator selecting the registry Factory ("feishu",
    /// "slack", …).
    pub channel_type: cordy_channel::Type,
    /// Opaque condensation of the credential-bearing config. Two rows
    /// with equal fingerprints are interchangeable to the supervisor; any
    /// change between sweeps tears the running connection down and
    /// rebuilds it so a re-installed channel (fresh app_id / secret /
    /// region) is picked up instead of running indefinitely against stale
    /// credentials. The store computes it; the engine treats it as
    /// opaque.
    pub fingerprint: String,
    /// The platform credential/config blob, passed verbatim to the
    /// registry Factory as `ChannelConfig::raw`. The engine never reads
    /// inside it.
    pub config: serde_json::Value,
}

/// Enumerates active installations across every channel type. Deliberately
/// separate from [`LeaseStore`] so production can keep installation
/// metadata in PostgreSQL while using Redis for low-churn leases.
#[async_trait]
pub trait InstallationStore: Send + Sync {
    /// Returns every active installation across ALL channel types. There
    /// is no per-platform filter here — that hard-coded "feishu" was the
    /// whole limitation PB-3620 removes.
    async fn list_active_installations(&self) -> anyhow::Result<Vec<Installation>>;
}

/// Backend-agnostic observability seam for lease operations.
///
/// Port note: Go checks `cfg.LeaseMetrics != nil` before each call; Rust
/// uses `Option<Arc<dyn LeaseMetrics>>`.
#[allow(unused_variables)]
pub trait LeaseMetrics: Send + Sync {
    fn record_lease_operation(&self, operation: &str, outcome: &str) {}
    fn set_active_lease_owners(&self, count: f64) {}
    fn set_owners_with_renewal_errors(&self, count: f64) {}
    fn set_last_successful_renewal(&self, at: DateTime<Utc>) {}
    fn observe_takeover_latency(&self, delay: Duration) {}
}

/// Tunes the Supervisor's lifecycle loops. Zero-valued fields get
/// production defaults via [`with_defaults`](Self::with_defaults); tests
/// inject `now` for determinism.
#[derive(Clone, Default)]
pub struct SupervisorConfig {
    /// How long a successful lease grant is valid before another replica
    /// may steal it. Renewals happen on the tighter lease_renew_interval;
    /// the gap absorbs transient DB blips.
    pub lease_ttl: Duration,
    /// Cadence at which the Supervisor re-acquires leases it already
    /// owns. MUST be substantially less than lease_ttl so a single missed
    /// renewal does not yield the lease.
    pub lease_renew_interval: Duration,
    /// How often the Supervisor scans for installations to take over (new
    /// ones, or ones whose lease expired on another replica).
    pub poll_interval: Duration,
    /// Fast retry cadence after a renewal transport error.
    pub lease_error_retry_interval: Duration,
    /// Subtracted from the last confirmed TTL so a partitioned owner
    /// disconnects before Redis can grant a successor.
    pub lease_expiry_safety_margin: Duration,
    /// Reconnect schedule bounds: start at min_backoff, double after each
    /// consecutive failure (capped at max_backoff), reset on any
    /// connection that lived at least reset_backoff_after.
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    pub reset_backoff_after: Duration,
    /// Caps a single lease release. It runs on a fresh context (the parent
    /// ctx is already cancelled by shutdown time), so without a deadline a
    /// frozen pool could hang shutdown indefinitely; on timeout the lease
    /// falls back to natural TTL expiry for the next replica.
    pub lease_release_timeout: Duration,
    /// Caps a single Channel disconnect made after a connection ends.
    pub disconnect_timeout: Duration,
    /// Bounds how long one sweep waits for all cancelled credential-
    /// rotation predecessors to disconnect and release their leases. One
    /// shared deadline covers the whole batch.
    pub rotation_wait_timeout: Duration,
    /// Bounds how long callers should wait for supervisors after cancel;
    /// exposed so boot and tests share a default.
    pub shutdown_timeout: Duration,
    /// Clock injection point; `None` uses the wall clock.
    pub now: Option<fn() -> DateTime<Utc>>,
}

fn dmin(a: Duration, b: Duration) -> Duration {
    if a < b {
        a
    } else {
        b
    }
}

impl SupervisorConfig {
    /// Fills zero-valued fields with the Go defaults (withDefaults).
    ///
    /// Port note: Go panics on invalid configs here; Rust returns Err and
    /// the constructor propagates, keeping boot failures explicit.
    pub fn with_defaults(mut self) -> Result<Self, anyhow::Error> {
        if self.lease_ttl.is_zero() {
            self.lease_ttl = Duration::from_secs(180);
        }
        if self.lease_renew_interval.is_zero() {
            self.lease_renew_interval = dmin(Duration::from_secs(60), self.lease_ttl / 3);
        }
        if self.poll_interval.is_zero() {
            self.poll_interval = dmin(Duration::from_secs(30), self.lease_renew_interval / 2);
        }
        if self.lease_error_retry_interval.is_zero() {
            self.lease_error_retry_interval =
                dmin(Duration::from_secs(5), self.lease_renew_interval / 4);
        }
        if self.lease_expiry_safety_margin.is_zero() {
            self.lease_expiry_safety_margin = dmin(Duration::from_secs(5), self.lease_ttl / 10);
        }
        if self.min_backoff.is_zero() {
            self.min_backoff = Duration::from_secs(2);
        }
        if self.max_backoff.is_zero() {
            self.max_backoff = Duration::from_secs(60);
        }
        if self.reset_backoff_after.is_zero() {
            self.reset_backoff_after = Duration::from_secs(60);
        }
        if self.lease_release_timeout.is_zero() {
            self.lease_release_timeout = Duration::from_secs(5);
        }
        if self.disconnect_timeout.is_zero() {
            self.disconnect_timeout = Duration::from_secs(5);
        }
        if self.rotation_wait_timeout.is_zero() {
            self.rotation_wait_timeout = self.disconnect_timeout + self.lease_release_timeout;
        }
        if self.shutdown_timeout.is_zero() {
            self.shutdown_timeout = Duration::from_secs(15);
        }
        self.validate()?;
        Ok(self)
    }

    /// Enforces the timing invariant required for safe fail-closed lease
    /// renewal. Callers parsing deployment config can use it before
    /// constructing a Supervisor; the constructor also rejects invalid
    /// programmatic configs.
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        if self.poll_interval.is_zero()
            || self.lease_renew_interval.is_zero()
            || self.lease_ttl.is_zero()
        {
            anyhow::bail!("channel engine: lease intervals must be positive");
        }
        if self.poll_interval > self.lease_renew_interval
            || self.lease_renew_interval >= self.lease_ttl
        {
            anyhow::bail!(
                "channel engine: require poll <= renew < ttl (poll={:?} renew={:?} ttl={:?})",
                self.poll_interval,
                self.lease_renew_interval,
                self.lease_ttl
            );
        }
        if self.lease_error_retry_interval.is_zero() {
            anyhow::bail!("channel engine: lease error retry interval must be positive");
        }
        if self.lease_expiry_safety_margin.is_zero()
            || self.lease_expiry_safety_margin >= self.lease_ttl - self.lease_renew_interval
        {
            anyhow::bail!(
                "channel engine: lease expiry safety margin must be positive and less than ttl-renew (margin={:?})",
                self.lease_expiry_safety_margin
            );
        }
        Ok(())
    }

    fn now(&self) -> DateTime<Utc> {
        match self.now {
            Some(f) => f(),
            None => Utc::now(),
        }
    }

    fn ttl_chrono(&self) -> chrono::Duration {
        chrono::Duration::from_std(self.lease_ttl).unwrap_or_default()
    }
}

struct SupervisorEntry {
    cancel: CancellationToken,
    done: tokio::sync::oneshot::Receiver<()>,
    abort: SharedAbortHandle,
    fingerprint: String,
    gen: u64,
}

struct SupervisorWait {
    done: tokio::sync::oneshot::Receiver<()>,
    abort: SharedAbortHandle,
}

impl SupervisorEntry {
    fn into_wait(self) -> SupervisorWait {
        SupervisorWait {
            done: self.done,
            abort: self.abort,
        }
    }
}

fn abort_waits(waits: &[SharedAbortHandle]) {
    for abort in waits {
        if let Some(abort) = abort
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            abort.abort();
        }
    }
}

struct Inner {
    supervisors: HashMap<String, SupervisorEntry>,
    /// When this node first observed a foreign owner, so successful
    /// expiry takeover latency can be measured without per-ID labels.
    contended_since: HashMap<String, DateTime<Utc>>,
    active_owners: i64,
    /// Bounded set (active local owners only) exposing partial renewal
    /// failure that a process-wide "last success" timestamp can otherwise
    /// hide when some other installation remains healthy.
    renewal_errors: HashSet<String>,
    stopped: bool,
}

fn cancel_revoked_supervisors(inner: &mut Inner, active: &HashSet<String>) {
    for (id, entry) in &inner.supervisors {
        if !active.contains(id) {
            entry.cancel.cancel();
        }
    }
    // Keep cancelled entries owned until their task sends `done` and removes
    // its generation. Dropping them here would also drop the only completion
    // receiver, so a concurrent process shutdown could no longer wait for
    // disconnect and token-fenced lease release to finish.
    inner.contended_since.retain(|id, _| active.contains(id));
}

/// Owns the per-installation supervisor tasks that keep a long-running
/// connection per active installation, across every channel type. It
/// enforces the multi-replica safety rule via the WS lease CAS — at most
/// one Supervisor globally holds the lease for any installation, so
/// duplicate event consumption across replicas is impossible.
pub struct Supervisor<S: InstallationStore, L: LeaseStore> {
    store: Arc<S>,
    lease_store: Arc<L>,
    registry: Arc<Registry>,
    handler: cordy_channel::InboundHandler,
    cfg: SupervisorConfig,
    metrics: Option<Arc<dyn LeaseMetrics>>,
    /// Per-process lease ownership token prefix. Matching tokens read as
    /// "this is us, renew", so a stable node_id keeps renewals from
    /// ping-ponging between replicas.
    node_id: String,
    inner: Mutex<Inner>,
    supervisor_gen: AtomicU64,
}

/// Keeps process-local lease gauges exact even when the owning async task is
/// force-aborted after its graceful shutdown deadline. Token-fenced release is
/// still attempted on every normal path; an aborted path deliberately relies
/// on backend TTL expiry, but must stop reporting itself as a live owner.
struct ActiveOwnerGuard<'a, S: InstallationStore + 'static, L: LeaseStore + 'static> {
    supervisor: &'a Supervisor<S, L>,
    installation_id: &'a str,
    active: bool,
}

impl<'a, S: InstallationStore + 'static, L: LeaseStore + 'static> ActiveOwnerGuard<'a, S, L> {
    fn new(supervisor: &'a Supervisor<S, L>, installation_id: &'a str) -> Self {
        supervisor.adjust_active_owners(1);
        Self {
            supervisor,
            installation_id,
            active: true,
        }
    }

    fn finish(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.supervisor
            .set_renewal_error(self.installation_id, false);
        self.supervisor.adjust_active_owners(-1);
    }
}

impl<S: InstallationStore + 'static, L: LeaseStore + 'static> Drop for ActiveOwnerGuard<'_, S, L> {
    fn drop(&mut self) {
        self.finish();
    }
}

impl<S: InstallationStore + 'static, L: LeaseStore + 'static> Supervisor<S, L> {
    /// Constructs a Supervisor bound to the supplied store, channel
    /// registry, and shared inbound handler. The handler is injected into
    /// every Channel built (via `ChannelConfig::handler`) so the inbound
    /// pipeline is written once and shared across platforms. No tasks are
    /// started until [`run_owned`] is called.
    pub fn new(
        store: Arc<S>,
        lease_store: Arc<L>,
        registry: Arc<Registry>,
        handler: cordy_channel::InboundHandler,
        cfg: SupervisorConfig,
        metrics: Option<Arc<dyn LeaseMetrics>>,
    ) -> Result<Arc<Self>, anyhow::Error> {
        let cfg = cfg.with_defaults()?;
        Ok(Arc::new(Self {
            store,
            lease_store,
            registry,
            handler,
            cfg,
            metrics,
            node_id: new_node_id(),
            inner: Mutex::new(Inner {
                supervisors: HashMap::new(),
                contended_since: HashMap::new(),
                active_owners: 0,
                renewal_errors: HashSet::new(),
                stopped: false,
            }),
            supervisor_gen: AtomicU64::new(0),
        }))
    }

    /// Exposes the per-process lease token, for tests and observability
    /// (so operators can correlate DB lease rows to a running replica).
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Exposes the configured graceful-shutdown deadline so boot can pass
    /// the same value to bounded waits without re-deriving it.
    pub fn shutdown_timeout(&self) -> Duration {
        self.cfg.shutdown_timeout
    }

    /// The Supervisor's main loop. Scans installations every poll_interval,
    /// attempts to lease any not currently supervised by this process, and
    /// reaps supervisors for installations that were revoked or whose
    /// lease was lost. Returns when `ctx` is cancelled.
    ///
    /// Port note: takes `Arc<Self>` so the detached supervise tasks can
    /// keep a valid handle for their whole lifetime (Go relied on the GC).
    pub async fn run_owned(self: Arc<Self>, ctx: CancellationToken) {
        // First sweep immediately so a freshly-restarted server doesn't
        // wait a full poll_interval before picking up its installations.
        self.run_once(&ctx).await;

        let mut ticker = tokio::time::interval(self.cfg.poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ctx.cancelled() => {
                    self.cancel_all_and_wait().await;
                    return;
                }
                _ = ticker.tick() => {
                    self.run_once(&ctx).await;
                }
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Enumerates currently-active installations and starts a supervisor
    /// for any this process does not yet supervise. Supervisors for
    /// revoked installations are cancelled. Supervisors whose installation
    /// row rotated credentials are cancelled and replaced inline so the
    /// new channel picks up the fresh row.
    /// Runs one deterministic discovery/acquisition pass. Production calls
    /// this from the polling loop; tests and maintenance tooling can inject a
    /// clock and exercise one pass without owning a background task.
    pub async fn run_once(self: &Arc<Self>, ctx: &CancellationToken) {
        let rows = match self.store.list_active_installations().await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "channel engine: list active installations failed");
                return;
            }
        };
        let mut active: HashSet<String> = HashSet::with_capacity(rows.len());
        let mut candidates: Vec<(Installation, bool)> = Vec::with_capacity(rows.len());
        let mut candidate_ids: Vec<uuid::Uuid> = Vec::with_capacity(rows.len());
        let mut rotation_waits: Vec<(String, SupervisorWait)> = Vec::new();
        for row in rows {
            // Skip channel types with no registered per-installation
            // Factory. Such rows are driven outside the Supervisor (e.g.
            // Slack's app-level Socket Mode connector owns ONE deployment
            // connection for all its installations). Without this guard
            // the supervise loop would acquire the lease, hit UnknownType
            // from Registry.build, release, and back off forever —
            // churning the lease and the log on every such row.
            if self.registry.lookup(&row.channel_type).is_none() {
                continue;
            }
            let id = row.id.to_string();
            active.insert(id.clone());
            let (done, rotated) = self.cancel_on_rotation(&id, &row);
            if rotated {
                rotation_waits.push((id, done.unwrap()));
            }
            if !self.is_supervised(&row.id.to_string()) {
                let supervised = false;
                let _ = supervised;
                candidates.push((row.clone(), rotated));
                candidate_ids.push(row.id);
            }
        }
        // Cancel supervisors whose installation is no longer active
        // (revoked since the last sweep). Their entries remain owned until
        // the task exits so process shutdown can still await cleanup.
        {
            let mut inner = self.lock();
            cancel_revoked_supervisors(&mut inner, &active);
        }

        if candidates.is_empty() {
            return;
        }
        self.wait_for_rotations(ctx, rotation_waits).await;
        let held = match self.lease_store.list_held(&candidate_ids).await {
            Ok(held) => {
                self.record_lease_operation("list", "success");
                held
            }
            Err(err) => {
                self.record_lease_operation("list", "error");
                tracing::warn!(
                    error = %err,
                    "channel engine: list held leases failed; acquisition sweep skipped"
                );
                return;
            }
        };
        for (row, local_rotation) in candidates {
            let id = row.id.to_string();
            if held.contains(&id) {
                if !local_rotation {
                    self.mark_contended(&id);
                }
                continue;
            }
            self.start_supervisor(ctx, row);
        }
    }

    fn is_supervised(&self, id: &str) -> bool {
        self.lock().supervisors.contains_key(id)
    }

    /// Cancels an existing supervisor when its fingerprint differs from
    /// the current row's and returns its completion signal. Sweep cancels
    /// every rotated predecessor first, then waits for the batch under one
    /// bounded deadline before checking held keys. A promptly-cancelled
    /// predecessor releases its key in time for its replacement to start
    /// in the same sweep.
    fn cancel_on_rotation(&self, id: &str, row: &Installation) -> (Option<SupervisorWait>, bool) {
        let want = row.fingerprint.clone();
        let mut inner = self.lock();
        let matches_fingerprint = inner
            .supervisors
            .get(id)
            .map(|e| e.fingerprint == want)
            .unwrap_or(true);
        if !inner.supervisors.contains_key(id) || matches_fingerprint {
            return (None, false);
        }
        tracing::info!(
            installation_id = %id,
            channel_type = %row.channel_type,
            "channel engine: credentials rotated, restarting supervisor"
        );
        let entry = inner.supervisors.remove(id).unwrap();
        entry.cancel.cancel();
        (Some(entry.into_wait()), true)
    }

    async fn wait_for_rotations(
        &self,
        ctx: &CancellationToken,
        waits: Vec<(String, SupervisorWait)>,
    ) {
        if waits.is_empty() {
            return;
        }
        let aborts = waits
            .iter()
            .map(|(_, wait)| wait.abort.clone())
            .collect::<Vec<_>>();
        let deadline = std::time::Instant::now() + self.cfg.rotation_wait_timeout;
        for (installation_id, mut wait) in waits {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                tracing::warn!(
                    timeout = ?self.cfg.rotation_wait_timeout,
                    "channel engine: timed out waiting for credential rotations to stop"
                );
                abort_waits(&aborts);
                self.record_lease_operation("rotation", "forced_abort");
                return;
            }
            tokio::select! {
                _ = ctx.cancelled() => {
                    abort_waits(&aborts);
                    return;
                },
                r = &mut wait.done => {
                    let _ = r; // predecessor finished or dropped
                }
                _ = tokio::time::sleep(remaining) => {
                    tracing::warn!(
                        installation_id = %installation_id,
                        timeout = ?self.cfg.rotation_wait_timeout,
                        "channel engine: timed out waiting for rotated supervisor to stop"
                    );
                    abort_waits(&aborts);
                    self.record_lease_operation("rotation", "forced_abort");
                    return;
                }
            }
        }
    }

    fn start_supervisor(self: &Arc<Self>, parent: &CancellationToken, inst: Installation) {
        let id = inst.id.to_string();
        let gen;
        let cancel;
        let done_tx;
        let abort = Arc::new(Mutex::new(None));
        {
            let mut inner = self.lock();
            if inner.stopped || inner.supervisors.contains_key(&id) {
                return;
            }
            gen = self.supervisor_gen.fetch_add(1, Ordering::SeqCst) + 1;
            cancel = parent.child_token();
            let (tx, done_rx) = tokio::sync::oneshot::channel::<()>();
            done_tx = tx;
            inner.supervisors.insert(
                id.clone(),
                SupervisorEntry {
                    cancel: cancel.clone(),
                    done: done_rx,
                    abort: abort.clone(),
                    fingerprint: inst.fingerprint.clone(),
                    gen,
                },
            );
        }
        let this = Arc::clone(self);
        let lease_tok = lease_token(&self.node_id, gen);
        let inst_id = inst.id;
        let task = tokio::spawn(async move {
            // A supervisor can exit without an explicit cancellation when
            // lease acquisition is contended or fails. Always detach its
            // child token when the task returns (Go: defer cancel()).
            let _ = &cancel;
            this.supervise(cancel, inst, id.clone(), gen, lease_tok)
                .await;
            // Signal completion; also clear our map entry if it still
            // belongs to us — gen disambiguates "this entry is mine" from
            // "the rotation path already replaced me".
            let mut inner = this.lock();
            if let Some(entry) = inner.supervisors.get(&id) {
                if entry.gen == gen {
                    inner.supervisors.remove(&id);
                }
            }
            drop(inner);
            let _ = done_tx.send(());
            let _ = inst_id;
        });
        *abort.lock().unwrap_or_else(|error| error.into_inner()) = Some(task.abort_handle());
    }

    /// Owns one installation's connection lifecycle. Loops: acquire lease
    /// → build channel → run it (connect blocks) → renew lease while it
    /// runs → on exit, release + back off → repeat. Returns when ctx is
    /// cancelled.
    #[allow(clippy::too_many_lines)]
    async fn supervise(
        self: &Arc<Self>,
        ctx: CancellationToken,
        inst: Installation,
        id: String,
        _gen: u64,
        lease_tok: String,
    ) {
        let (done_tx, _) = tokio::sync::oneshot::channel::<()>();
        // The map holds the receiver; we own the sender side through the
        // entry we registered in start_supervisor. Because the entry was
        // moved out there, we simply signal nothing here — completion is
        // observed via generation-checked removal in the caller task.
        let _ = done_tx;

        let mut backoff = self.cfg.min_backoff;
        loop {
            if ctx.is_cancelled() {
                return;
            }

            // A losing candidate exits. The next batched sweep observes
            // Redis and starts a fresh attempt only after the key
            // disappears, avoiding a per-installation blind retry loop on
            // every replica.
            match self.acquire_lease(&inst.id, &lease_tok).await {
                Err(err) => {
                    self.record_lease_operation("acquire", "error");
                    tracing::warn!(error = %err, "channel engine: acquire lease error");
                    return;
                }
                Ok(None) => {
                    self.record_lease_operation("acquire", "contended");
                    self.mark_contended(&id);
                    return;
                }
                Ok(Some(_confirmed_until)) => {
                    self.record_lease_operation("acquire", "success");
                    self.observe_takeover(&id);
                    let mut active_owner = ActiveOwnerGuard::new(self, &id);

                    // Build the platform channel via the registry, run it
                    // under a child token, and renew the lease in parallel.
                    let run_ctx = ctx.child_token();
                    let generation = LeaseGeneration::new(lease_tok.clone(), run_ctx.clone());
                    let built: anyhow::Result<BuiltChannel> = self
                        .registry
                        .build(ChannelConfig {
                            r#type: inst.channel_type.clone(),
                            id: Some(inst.id),
                            raw: inst.config.clone(),
                            handler: Some(self.handler.clone()),
                            generation: Some(generation.clone()),
                        })
                        .await;
                    let ch = match built {
                        Ok(ch) => ch,
                        Err(err) => {
                            generation.revoke();
                            tracing::error!(
                                error = %err,
                                "channel engine: build channel failed"
                            );
                            self.release_lease(&inst.id, &lease_tok).await;
                            active_owner.finish();
                            if sleep(&ctx, backoff).await {
                                return;
                            }
                            backoff = next_backoff(backoff, self.cfg.max_backoff);
                            continue;
                        }
                    };

                    let renew_this = Arc::clone(self);
                    let renew_run = run_ctx.clone();
                    let renew_inst_id = inst.id;
                    let renew_token = lease_tok.clone();
                    let renewed = tokio::spawn(async move {
                        // renew_lease_until cancels run_ctx itself on lease
                        // loss so the channel exits even if its wire I/O is
                        // blocked. This is what makes "at most one active
                        // connection per installation across replicas"
                        // hold under lease theft.
                        renew_this
                            .renew_lease_until(
                                &run_ctx_of(&renew_run),
                                &renew_inst_id,
                                &renew_token,
                            )
                            .await;
                    });

                    let started_at = self.cfg.now();
                    let run_result = ch.connect(run_ctx.clone()).await;
                    generation.revoke();
                    let _ = renewed.await;
                    self.disconnect_channel(&ch, &id).await;
                    self.release_lease(&inst.id, &lease_tok).await;
                    active_owner.finish();

                    if ctx.is_cancelled() {
                        return;
                    }

                    // If the connection lived long enough to be "stable",
                    // reset the backoff so a single late failure does not
                    // start us at the cap.
                    let uptime = (self.cfg.now() - started_at).to_std().unwrap_or_default();
                    if uptime >= self.cfg.reset_backoff_after {
                        backoff = self.cfg.min_backoff;
                    }
                    match run_result {
                        Err(err) => tracing::warn!(
                            error = %err,
                            uptime = ?uptime,
                            "channel engine: connection exited with error"
                        ),
                        Ok(()) => tracing::info!(
                            uptime = ?uptime,
                            "channel engine: connection exited cleanly"
                        ),
                    }
                    if sleep(&ctx, jitter(backoff)).await {
                        return;
                    }
                    backoff = next_backoff(backoff, self.cfg.max_backoff);
                }
            }
        }
    }

    /// Tries to claim or renew the WS lease. Returns `Ok(Some(confirmed))`
    /// when owned after the call; `Ok(None)` when held elsewhere; Err for
    /// transport failures. Token is the per-supervisor token (see
    /// [`lease_token`]), NOT the process-wide node_id.
    async fn acquire_lease(
        &self,
        inst_id: &uuid::Uuid,
        token: &str,
    ) -> Result<Option<DateTime<Utc>>, LeaseError> {
        let started = self.cfg.now();
        let ttl = self.cfg.ttl_chrono();
        match self
            .lease_store
            .try_acquire(AcquireLeaseParams {
                id: *inst_id,
                token: token.to_string(),
                expires_at: started + ttl,
                ttl,
            })
            .await
        {
            Ok(()) => {}
            Err(LeaseError::NotAcquired) => return Ok(None),
            Err(error) => return Err(error),
        }
        Ok(Some(
            started + ttl
                - chrono::Duration::from_std(self.cfg.lease_expiry_safety_margin).unwrap(),
        ))
    }

    /// Re-acquires the lease on a tight cadence so a single missed
    /// renewal does not yield it. Exits when ctx is cancelled. Lease loss
    /// MUST cancel the channel's run context — otherwise the supervise
    /// loop would release the lease while the channel's receive loop kept
    /// consuming events until its wire I/O finally errored, exactly the
    /// "two replicas processing the same installation" failure mode.
    async fn renew_lease_until(
        self: &Arc<Self>,
        ctx: &CancellationToken,
        inst_id: &uuid::Uuid,
        token: &str,
    ) {
        let confirmed_until = match self.acquire_lease(inst_id, token).await {
            // Re-acquire is a renewal when we still own it (same token).
            Ok(Some(until)) => until,
            Ok(None) => {
                self.lease_lost(inst_id, ctx, "lease token no longer matches");
                return;
            }
            Err(err) => {
                tracing::warn!(error = %err, "channel engine: initial renewal probe failed");
                self.lease_lost(inst_id, ctx, "renewal probe error");
                return;
            }
        };
        let mut confirmed_until = confirmed_until;
        let mut next_delay = renewal_jitter(self.cfg.lease_renew_interval);
        loop {
            let remaining = (confirmed_until - self.cfg.now())
                .to_std()
                .unwrap_or_default();
            if remaining.is_zero() && confirmed_until <= self.cfg.now() {
                self.lease_lost(inst_id, ctx, "last confirmed lease expired");
                return;
            }
            let wait = dmin(next_delay, remaining.max(Duration::from_millis(1)));
            tokio::select! {
                _ = ctx.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
            if self.cfg.now() >= confirmed_until {
                self.lease_lost(inst_id, ctx, "last confirmed lease expired");
                return;
            }

            let started = self.cfg.now();
            let attempt_budget = (confirmed_until - started).to_std().unwrap_or_default();
            let ttl = self.cfg.ttl_chrono();
            let attempt = tokio::time::timeout(
                attempt_budget,
                self.lease_store.renew(AcquireLeaseParams {
                    id: *inst_id,
                    token: token.to_string(),
                    expires_at: started + ttl,
                    ttl,
                }),
            )
            .await;
            match attempt {
                // Budget exhausted reads as a lost window.
                Err(_) => {
                    self.lease_lost(inst_id, ctx, "renewal budget exceeded");
                    return;
                }
                Ok(Err(LeaseError::NotAcquired)) => {
                    self.lease_lost(inst_id, ctx, "lease token no longer matches");
                    return;
                }
                Ok(Err(err)) => {
                    self.set_renewal_error(&inst_id.to_string(), true);
                    self.record_lease_operation("renew", "error");
                    tracing::warn!(
                        installation_id = %inst_id,
                        error = %err,
                        confirmed_until = %confirmed_until,
                        "channel engine: lease renewal error"
                    );
                    next_delay = self.cfg.lease_error_retry_interval;
                    continue;
                }
                Ok(Ok(())) => {}
            }
            confirmed_until = started + ttl
                - chrono::Duration::from_std(self.cfg.lease_expiry_safety_margin).unwrap();
            self.set_renewal_error(&inst_id.to_string(), false);
            self.record_lease_operation("renew", "success");
            if let Some(m) = &self.metrics {
                m.set_last_successful_renewal(self.cfg.now());
            }
            next_delay = renewal_jitter(self.cfg.lease_renew_interval);
        }
    }

    fn lease_lost(&self, inst_id: &uuid::Uuid, run_ctx: &CancellationToken, reason: &str) {
        self.set_renewal_error(&inst_id.to_string(), false);
        self.record_lease_operation("renew", "lost");
        tracing::warn!(
            installation_id = %inst_id,
            reason = %reason,
            "channel engine: lease lost; tearing down connection"
        );
        run_ctx.cancel();
    }

    /// Writes a token-fenced release so the next supervisor (this process
    /// or another replica) can pick up the installation without waiting
    /// for LeaseTTL. Runs on a fresh bounded context (the parent ctx is
    /// already cancelled by shutdown time). A rotation successor's lease
    /// carries a different token, so a stale release no-ops instead of
    /// clobbering it.
    async fn release_lease(&self, inst_id: &uuid::Uuid, token: &str) {
        self.set_renewal_error(&inst_id.to_string(), false);
        let release = self.lease_store.release(ReleaseLeaseParams {
            id: *inst_id,
            token: token.to_string(),
        });
        match tokio::time::timeout(self.cfg.lease_release_timeout, release).await {
            Ok(Ok(())) => self.record_lease_operation("release", "success"),
            Ok(Err(err)) => {
                self.record_lease_operation("release", "error");
                tracing::warn!(
                    installation_id = %inst_id,
                    error = %err,
                    "channel engine: release lease failed"
                );
            }
            Err(_) => {
                self.record_lease_operation("release", "error");
                tracing::warn!(
                    installation_id = %inst_id,
                    "channel engine: release lease timed out"
                );
            }
        }
    }

    fn record_lease_operation(&self, operation: &str, outcome: &str) {
        if let Some(m) = &self.metrics {
            m.record_lease_operation(operation, outcome);
        }
    }

    fn adjust_active_owners(&self, delta: i64) {
        let count = {
            let mut inner = self.lock();
            inner.active_owners += delta;
            inner.active_owners
        };
        if let Some(m) = &self.metrics {
            m.set_active_lease_owners(count as f64);
        }
    }

    fn set_renewal_error(&self, id: &str, has_error: bool) {
        let (already_set, count) = {
            let mut inner = self.lock();
            let already_set = inner.renewal_errors.contains(id);
            if has_error {
                inner.renewal_errors.insert(id.to_string());
            } else {
                inner.renewal_errors.remove(id);
            }
            (already_set, inner.renewal_errors.len())
        };
        if already_set == has_error {
            return;
        }
        if let Some(m) = &self.metrics {
            m.set_owners_with_renewal_errors(count as f64);
        }
    }

    fn mark_contended(&self, id: &str) {
        let mut inner = self.lock();
        inner
            .contended_since
            .entry(id.to_string())
            .or_insert_with(|| self.cfg.now());
    }

    fn observe_takeover(&self, id: &str) {
        let started = {
            let mut inner = self.lock();
            inner.contended_since.remove(id)
        };
        if let Some(started) = started {
            if let Some(m) = &self.metrics {
                let delay = (self.cfg.now() - started).to_std().unwrap_or_default();
                m.observe_takeover_latency(delay);
            }
        }
    }

    /// Tears down a channel after its connect returned, on a fresh bounded
    /// timeout so a wedged disconnect cannot hang the supervise loop. By
    /// the time we get here the link is already down (connect returned),
    /// so this is best-effort resource cleanup.
    async fn disconnect_channel(&self, ch: &BuiltChannel, id: &str) {
        match tokio::time::timeout(self.cfg.disconnect_timeout, ch.disconnect()).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!(
                installation_id = %id,
                error = %err,
                "channel engine: disconnect failed"
            ),
            Err(_) => tracing::warn!(
                installation_id = %id,
                "channel engine: disconnect timed out"
            ),
        }
    }

    async fn cancel_all_and_wait(&self) {
        let waits = {
            let mut inner = self.lock();
            inner.stopped = true;
            inner
                .supervisors
                .drain()
                .map(|(_, entry)| {
                    entry.cancel.cancel();
                    entry.into_wait()
                })
                .collect::<Vec<_>>()
        };
        let aborts = waits
            .iter()
            .map(|wait| wait.abort.clone())
            .collect::<Vec<_>>();
        let join = async {
            for wait in waits {
                let _ = wait.done.await;
            }
        };
        if tokio::time::timeout(self.cfg.shutdown_timeout, join)
            .await
            .is_err()
        {
            abort_waits(&aborts);
            self.record_lease_operation("shutdown", "forced_abort");
            tracing::warn!(
                timeout = ?self.cfg.shutdown_timeout,
                "channel supervisor: connections did not exit before shutdown deadline; aborted"
            );
        }
    }
}

fn run_ctx_of(t: &CancellationToken) -> CancellationToken {
    t.clone()
}

/// Composes the per-supervisor lease token: the process-wide node_id (for
/// cross-replica observability) paired with the supervisor's gen so two
/// supervisors inside the SAME process running back-to-back for the same
/// installation (the rotation path) carry different tokens. That
/// distinction stops an old supervisor's post-cancel release from
/// CAS-matching and deleting the successor's just-acquired lease.
///
/// The result is an internal CAS marker, NOT a credential: it is never
/// sent to any platform and on its own grants nothing. It is still kept
/// out of log FIELDS — GH #7132 reported a plaintext `lease_token=` field
/// as a leaked channel credential; log node_id + lease_gen instead.
pub(crate) fn lease_token(node_id: &str, gen: u64) -> String {
    format!("{node_id}-g{gen}")
}

/// Doubles the current backoff up to max.
pub(crate) fn next_backoff(cur: Duration, max: Duration) -> Duration {
    let next = cur * 2;
    if next > max {
        max
    } else {
        next
    }
}

/// Spreads reconnect storms across the [0.5d, 1.5d) window so many
/// installations do not all retry on the same timer edge.
pub(crate) fn jitter(d: Duration) -> Duration {
    use rand::Rng;
    let millis = d.as_millis() as u64;
    if millis == 0 {
        return d;
    }
    let delta = millis / 2;
    if delta == 0 {
        return d;
    }
    let spread = rand::thread_rng().gen_range(0..(2 * delta));
    Duration::from_millis(millis - delta + spread)
}

/// Uses a narrow [0.9d, 1.1d] window so replicas spread their Redis calls
/// without eroding the configured TTL safety budget.
pub(crate) fn renewal_jitter(d: Duration) -> Duration {
    use rand::Rng;
    let millis = d.as_millis() as u64;
    if millis == 0 {
        return d;
    }
    let delta = millis / 10;
    if delta == 0 {
        return d;
    }
    let spread = rand::thread_rng().gen_range(0..=(2 * delta));
    Duration::from_millis(millis - delta + spread)
}

/// Cancellation-aware sleep. Returns true iff ctx was cancelled before
/// the sleep completed, so callers can short-circuit shutdown.
async fn sleep(ctx: &CancellationToken, d: Duration) -> bool {
    if d.is_zero() {
        return ctx.is_cancelled();
    }
    tokio::select! {
        _ = ctx.cancelled() => true,
        _ = tokio::time::sleep(d) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopStore;

    #[async_trait]
    impl InstallationStore for NoopStore {
        async fn list_active_installations(&self) -> anyhow::Result<Vec<Installation>> {
            Ok(Vec::new())
        }
    }

    struct NoopLease;

    #[async_trait]
    impl LeaseStore for NoopLease {
        async fn list_held(&self, _ids: &[uuid::Uuid]) -> Result<HashSet<String>, LeaseError> {
            Ok(HashSet::new())
        }

        async fn try_acquire(&self, _arg: AcquireLeaseParams) -> Result<(), LeaseError> {
            Ok(())
        }

        async fn renew(&self, _arg: AcquireLeaseParams) -> Result<(), LeaseError> {
            Ok(())
        }

        async fn release(&self, _arg: ReleaseLeaseParams) -> Result<(), LeaseError> {
            Ok(())
        }
    }

    fn test_supervisor() -> Arc<Supervisor<NoopStore, NoopLease>> {
        Supervisor::new(
            Arc::new(NoopStore),
            Arc::new(NoopLease),
            Arc::new(Registry::new()),
            cordy_channel::InboundHandler::new(|_, _| Box::pin(async { Ok(()) })),
            SupervisorConfig::default(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn active_owner_guard_cleans_up_once_on_finish_or_drop() {
        let supervisor = test_supervisor();
        {
            let mut guard = ActiveOwnerGuard::new(supervisor.as_ref(), "installation");
            assert_eq!(supervisor.lock().active_owners, 1);
            guard.finish();
            guard.finish();
            assert_eq!(supervisor.lock().active_owners, 0);
        }
        {
            let _guard = ActiveOwnerGuard::new(supervisor.as_ref(), "installation");
            assert_eq!(supervisor.lock().active_owners, 1);
        }
        assert_eq!(supervisor.lock().active_owners, 0);
    }

    #[tokio::test]
    async fn abort_waits_terminates_a_tracked_supervisor_task() {
        let task = tokio::spawn(std::future::pending::<()>());
        let abort = Arc::new(Mutex::new(Some(task.abort_handle())));

        abort_waits(&[abort]);

        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn revoked_supervisor_remains_owned_until_task_completion() {
        let cancel = CancellationToken::new();
        let (_done_tx, done) = tokio::sync::oneshot::channel();
        let mut inner = Inner {
            supervisors: HashMap::from([(
                "revoked".to_owned(),
                SupervisorEntry {
                    cancel: cancel.clone(),
                    done,
                    abort: Arc::new(Mutex::new(None)),
                    fingerprint: "old".to_owned(),
                    gen: 1,
                },
            )]),
            contended_since: HashMap::from([("revoked".to_owned(), Utc::now())]),
            active_owners: 0,
            renewal_errors: HashSet::new(),
            stopped: false,
        };

        cancel_revoked_supervisors(&mut inner, &HashSet::new());

        assert!(cancel.is_cancelled());
        assert!(inner.supervisors.contains_key("revoked"));
        assert!(!inner.contended_since.contains_key("revoked"));
    }

    #[test]
    fn lease_token_composes_node_and_generation() {
        assert_eq!(lease_token("abc123", 7), "abc123-g7");
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(
            next_backoff(Duration::from_secs(2), Duration::from_secs(60)),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(40), Duration::from_secs(60)),
            Duration::from_secs(60)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(64), Duration::from_secs(60)),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn jitter_stays_in_half_window() {
        let d = Duration::from_secs(10);
        for _ in 0..200 {
            let j = jitter(d);
            assert!(
                j >= Duration::from_secs(5) && j < Duration::from_secs(15),
                "{j:?}"
            );
        }
        assert_eq!(jitter(Duration::from_millis(1)), Duration::from_millis(1));
    }

    #[test]
    fn renewal_jitter_stays_narrow() {
        let d = Duration::from_secs(10);
        for _ in 0..200 {
            let j = renewal_jitter(d);
            assert!(
                j >= Duration::from_secs(9) && j <= Duration::from_secs(11),
                "{j:?}"
            );
        }
        // Sub-tick durations pass through unchanged (delta rounds to 0).
        assert_eq!(
            renewal_jitter(Duration::from_millis(5)),
            Duration::from_millis(5)
        );
    }

    #[test]
    fn config_defaults_satisfy_timing_invariant() {
        let cfg = SupervisorConfig::default().with_defaults().unwrap();
        assert_eq!(cfg.lease_ttl, Duration::from_secs(180));
        assert_eq!(cfg.lease_renew_interval, Duration::from_secs(60));
        assert_eq!(cfg.poll_interval, Duration::from_secs(30));
        assert_eq!(cfg.lease_error_retry_interval, Duration::from_secs(5));
        assert_eq!(cfg.lease_expiry_safety_margin, Duration::from_secs(5));
        assert_eq!(cfg.rotation_wait_timeout, Duration::from_secs(10));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn config_rejects_poll_above_renew() {
        let cfg = SupervisorConfig {
            poll_interval: Duration::from_secs(10),
            lease_renew_interval: Duration::from_secs(5),
            lease_ttl: Duration::from_secs(30),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("poll <= renew < ttl"));
    }

    #[test]
    fn config_rejects_margin_overlapping_renew_window() {
        let cfg = SupervisorConfig {
            poll_interval: Duration::from_secs(5),
            lease_renew_interval: Duration::from_secs(10),
            lease_ttl: Duration::from_secs(30),
            lease_error_retry_interval: Duration::from_secs(5),
            lease_expiry_safety_margin: Duration::from_secs(21),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("ttl-renew"));
    }
}
