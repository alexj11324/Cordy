//! Manager — the outbound GitHub API refresh pipeline orchestration.
//!
//! Port of `server/internal/integrations/ghsnapshot/refresh.go`. The manager
//! owns the queue / worker pool, the per-address dedup + single-in-flight +
//! trailing-edge state machine, installation-scoped rate-limit pauses, the
//! bounded chase-window backoff, and the TTL sweeper.
//!
//! Go → Rust mapping:
//! - goroutine pool + `chan address` → tokio tasks over an mpsc channel
//!   (buffer 2048); `Enqueue` stays non-blocking via `try_send`.
//! - `context.Context` cancellation → [`tokio_util::sync::CancellationToken`]
//!   (obtain it via [`Manager::shutdown_token`] to stop the pipeline).
//! - `time.AfterFunc` timers → spawned sleeps that re-check the token.
//! - `TxBeginner` + `*db.Queries` → a single sqlx `PgPool`; the cordy-db
//!   free functions take the transaction as executor.
//! - `pgtype.Text/Timestamptz` nullability → `Option<&str>` /
//!   `Option<DateTime<Utc>>`.
//! - `MaybeEnqueueOnView(installationID, owner, repo, number, fetchedAt
//!   time.Time, hasFetched bool)` collapses `(fetchedAt, hasFetched)` into a
//!   single `Option<DateTime<Utc>>` (`None` == never fetched).
//!
//! Credential hygiene: fetch errors are logged without secrets; the client
//! layer guarantees no token material ever reaches an error message.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use uuid::Uuid;

use cordy_db::queries::github_snapshot as dbq;

use crate::client::{Client, RateLimitError};
use crate::snapshot::{fetch_pr_snapshot, PrSnapshot};

/// Default chase-window backoff: climbs 30s → 1m → 2m → 5m and holds at 5m;
/// a chase stops when the snapshot is decided, the PR closes, or
/// [`MAX_CHASE_ATTEMPTS`] is reached — never unbounded. The TTL sweep and
/// page-visit refresh recover anything a stopped chase misses.
pub const DEFAULT_CHASE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
];

const DEFAULT_CONCURRENCY: usize = 12;
const DEFAULT_VIEW_TTL: Duration = Duration::from_secs(60);
const DEFAULT_SWEEP_TTL: Duration = Duration::from_secs(10 * 60);
const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DEFAULT_SWEEP_MAX_ROWS: i32 = 200;
/// Bounded chase window: an endlessly-pending CI or a wedged mergeability
/// verdict can never spin forever.
const MAX_CHASE_ATTEMPTS: u32 = 12;
const QUEUE_BUFFER: usize = 2048;
const MAX_JITTER_MS: u64 = 250;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The refresh unit and the dedup / single-in-flight key: one (installation,
/// owner, repo, number) tuple, which may fan out to multiple
/// github_pull_request rows across workspaces.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Address {
    pub installation_id: i64,
    pub owner: String,
    pub repo: String,
    pub number: i32,
}

type FetchFuture = Pin<Box<dyn Future<Output = anyhow::Result<PrSnapshot>> + Send>>;
/// The snapshot fetcher — a seam so tests can drive the queue / backoff
/// without a live GitHub. Defaults to [`fetch_pr_snapshot`].
type FetchFn = Arc<dyn Fn(Arc<Client>, Address) -> FetchFuture + Send + Sync>;
/// Called once per PR row whose snapshot was actually written (guard passed),
/// so the handler can broadcast a realtime PR update. Long-running work can
/// be spawned inside; do not block the worker.
pub type OnApplied = Arc<dyn Fn(Uuid) + Send + Sync>;

fn default_fetch() -> FetchFn {
    Arc::new(|client, addr| {
        Box::pin(async move {
            fetch_pr_snapshot(
                &client,
                addr.installation_id,
                &addr.owner,
                &addr.repo,
                addr.number,
            )
            .await
        })
    })
}

fn default_jitter() -> Box<dyn Fn() -> Duration + Send + Sync> {
    Box::new(|| {
        use rand::Rng;
        let ms = rand::thread_rng().gen_range(0..MAX_JITTER_MS);
        Duration::from_millis(ms)
    })
}

#[derive(Default)]
struct State {
    /// Queued OR in-flight → coalesce (single in-flight per PR).
    active: HashSet<Address>,
    in_flight: HashSet<Address>,
    /// One event that arrived while active; replay once after the current
    /// fetch.
    trailing: HashSet<Address>,
    /// Chase attempts for the current undecided window.
    attempts: HashMap<Address, u32>,
    /// Last address returned by the bounded TTL sweep. The query starts after
    /// this cursor and wraps, preventing a fixed first page from starving
    /// later installations when early addresses repeatedly fail.
    sweep_after: Address,
    /// Secondary rate limits are scoped to the installation whose token
    /// incurred them. One customer must never pause every other installation.
    rate_until: HashMap<i64, DateTime<Utc>>,
}

impl State {
    fn release(&mut self, addr: &Address) {
        self.active.remove(addr);
        self.in_flight.remove(addr);
        self.trailing.remove(addr);
    }
}

/// Owns the outbound refresh pipeline. A Manager whose client is `None`
/// is inert: every trigger method is a no-op, so a deployment without a
/// GitHub App private key degrades the feature off without touching PR
/// linking, merge→Done, or any other existing behavior.
pub struct Manager {
    client: Option<Arc<Client>>,
    pool: Option<PgPool>,
    on_applied: Option<OnApplied>,

    concurrency: usize,
    view_ttl: Duration,
    sweep_ttl: Duration,
    sweep_interval: Duration,
    sweep_max_rows: i32,
    chase_backoff: Vec<Duration>,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    jitter: Box<dyn Fn() -> Duration + Send + Sync>,
    fetch: FetchFn,

    queue_tx: mpsc::Sender<Address>,
    // Shared receiver: one address handed to exactly one worker at a time.
    queue_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Address>>>,

    state: Mutex<State>,
    cancel: tokio_util::sync::CancellationToken,
    started: std::sync::atomic::AtomicBool,
    accepting_tasks: std::sync::atomic::AtomicBool,
    tasks: Mutex<JoinSet<()>>,
}

impl Manager {
    /// Wires the pipeline. `on_applied` fires once per PR row whose snapshot
    /// was actually written (guard passed), so the handler can broadcast a
    /// realtime PR update. A `None` client yields a disabled (no-op) manager.
    pub fn new(
        client: Option<Client>,
        pool: Option<PgPool>,
        on_applied: Option<OnApplied>,
    ) -> Self {
        let (queue_tx, queue_rx) = mpsc::channel(QUEUE_BUFFER);
        Self {
            client: client.map(Arc::new),
            pool,
            on_applied,
            concurrency: DEFAULT_CONCURRENCY,
            view_ttl: DEFAULT_VIEW_TTL,
            sweep_ttl: DEFAULT_SWEEP_TTL,
            sweep_interval: DEFAULT_SWEEP_INTERVAL,
            sweep_max_rows: DEFAULT_SWEEP_MAX_ROWS,
            chase_backoff: DEFAULT_CHASE_BACKOFF.to_vec(),
            now: Box::new(Utc::now),
            jitter: default_jitter(),
            fetch: default_fetch(),
            queue_tx,
            queue_rx: Arc::new(tokio::sync::Mutex::new(queue_rx)),
            state: Mutex::new(State::default()),
            cancel: tokio_util::sync::CancellationToken::new(),
            started: std::sync::atomic::AtomicBool::new(false),
            accepting_tasks: std::sync::atomic::AtomicBool::new(false),
            tasks: Mutex::new(JoinSet::new()),
        }
    }

    /// Test seam: replaces the snapshot fetcher so queue/backoff behavior can
    /// be driven without a live GitHub.
    #[cfg(test)]
    pub(crate) fn set_fetch(&mut self, fetch: FetchFn) {
        self.fetch = fetch;
    }

    #[cfg(test)]
    async fn queued_len(&self) -> usize {
        self.queue_rx.lock().await.len()
    }

    /// Reports whether the pipeline will actually do anything.
    pub fn enabled(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.enabled())
    }

    pub fn client(&self) -> Option<Arc<Client>> {
        self.client.clone()
    }

    /// Token whose cancellation stops workers, the sweeper, and pending
    /// retry/defer timers. Mirrors cancelling the ctx given to Go's Start.
    pub fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel.clone()
    }

    /// Launches the worker pool and the TTL sweeper under the manager's
    /// shutdown token. No-op (and safe) when the manager is disabled or
    /// already started.
    pub fn start(
        self: &Arc<Self>,
        parent: tokio_util::sync::CancellationToken,
    ) -> Option<ManagerRuntime> {
        if !self.enabled() {
            return None;
        }
        if self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        self.accepting_tasks
            .store(true, std::sync::atomic::Ordering::Release);
        let m = Arc::clone(self);
        self.spawn_task(async move {
            tokio::select! {
                _ = parent.cancelled() => {}
                _ = m.cancel.cancelled() => {}
            }
            m.cancel.cancel();
        });
        for _ in 0..self.concurrency {
            let m = Arc::clone(self);
            self.spawn_task(m.worker());
        }
        let m = Arc::clone(self);
        self.spawn_task(m.sweep_loop());
        Some(ManagerRuntime {
            manager: Arc::clone(self),
        })
    }

    /// Schedules a refresh for a PR address. Repeated events coalesce, but
    /// an event that arrives while the address is queued or in flight leaves
    /// one trailing refresh behind. That trailing edge matters when a
    /// synchronize event advances the head while the old head's request is
    /// still running: the guarded old response is discarded, then the new
    /// head is fetched immediately. At most one request per address is in
    /// flight, and at most one trailing request is retained. Never blocks
    /// the caller.
    pub fn enqueue(
        &self,
        installation_id: i64,
        owner: impl Into<String>,
        repo: impl Into<String>,
        number: i32,
    ) {
        if !self.enabled() {
            return;
        }
        let addr = Address {
            installation_id,
            owner: owner.into(),
            repo: repo.into(),
            number,
        };
        {
            let mut st = self.lock_state();
            if st.active.contains(&addr) {
                if st.in_flight.contains(&addr) {
                    st.trailing.insert(addr);
                }
                return;
            }
            st.active.insert(addr.clone());
        }
        if let Err(err) = self.queue_tx.try_send(addr.clone()) {
            // Queue is saturated; drop and let the TTL sweep / next event
            // recover rather than block a webhook handler. Unmark so it can
            // be re-enqueued.
            self.lock_state().release(&addr);
            drop_marker_warn(err, "dropping enqueue");
        }
    }

    /// Page-visit trigger: refresh only when the snapshot is missing or older
    /// than the view TTL, so opening a card that already has fresh data costs
    /// nothing. `snapshot_fetched_at == None` means "never fetched".
    pub fn maybe_enqueue_on_view(
        &self,
        installation_id: i64,
        owner: impl Into<String>,
        repo: impl Into<String>,
        number: i32,
        snapshot_fetched_at: Option<DateTime<Utc>>,
    ) {
        if !self.enabled() {
            return;
        }
        if let Some(fetched_at) = snapshot_fetched_at {
            let cutoff_ms = (self.now)().timestamp_millis()
                - i64::try_from(self.view_ttl.as_millis()).unwrap_or(i64::MAX);
            if fetched_at.timestamp_millis() > cutoff_ms {
                return; // fresh (< view TTL)
            }
        }
        self.enqueue(installation_id, owner, repo, number);
    }

    async fn worker(self: Arc<Self>) {
        loop {
            let maybe_addr = {
                let mut rx = self.queue_rx.lock().await;
                tokio::select! {
                    _ = self.cancel.cancelled() => None,
                    addr = rx.recv() => addr,
                }
            };
            let Some(addr) = maybe_addr else { return };

            // A rate-limited installation waits outside the worker pool. Keep
            // the address active while a timer owns it, so events continue to
            // coalesce without letting one tenant occupy every global worker.
            let pause = self.rate_limit_pause(addr.installation_id);
            if pause > Duration::ZERO {
                self.defer_active(addr, pause);
                continue;
            }

            {
                let mut st = self.lock_state();
                st.in_flight.insert(addr.clone());
            }
            self.process(&addr).await;
            self.finish(addr);
        }
    }

    async fn process(self: &Arc<Self>, addr: &Address) {
        // Per-request jitter smooths bursts. Installation-scoped rate-limit
        // waits are handled before this point so they never consume a worker
        // slot.
        let j = (self.jitter)();
        if j > Duration::ZERO && !sleep_or_cancel(j, &self.cancel).await {
            return;
        }

        let Some(client) = self.client.clone() else {
            return;
        };
        let fetch = Arc::clone(&self.fetch);
        let snap = match fetch(client, addr.clone()).await {
            Ok(snap) => snap,
            Err(err) => {
                if let Some(rl) = err.downcast_ref::<RateLimitError>() {
                    // Do not create an unbounded retry loop for a persistently
                    // limited installation. The bounded TTL sweep (open/draft
                    // PRs only) or the next webhook/view event hands the
                    // address back after Retry-After.
                    self.extend_rate_limit(addr.installation_id, rl.retry_after);
                    return;
                }
                // Transient/GitHub failure: keep the last-known snapshot (the
                // row is untouched, so the card shows stale data, never wrong
                // data). No secret is ever logged. The next trigger or the TTL
                // sweep retries.
                tracing::warn!(
                    owner = %addr.owner,
                    repo = %addr.repo,
                    number = addr.number,
                    error = %err,
                    "ghsnapshot: fetch failed"
                );
                return;
            }
        };

        let Some(pool) = self.pool.as_ref() else {
            tracing::warn!("ghsnapshot: manager has no database pool; dropping snapshot");
            return;
        };

        let rows = match dbq::list_git_hub_pr_rows_by_address(
            pool,
            addr.installation_id,
            &addr.owner,
            &addr.repo,
            addr.number,
        )
        .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "ghsnapshot: list rows failed");
                return;
            }
        };

        let mut any_applied = false;
        let mut any_open_applied = false;
        for row in rows {
            let Some(pr_id) = row.id else { continue };
            match self.apply_snapshot(pr_id, &snap).await {
                Err(err) => {
                    tracing::warn!(error = %err, "ghsnapshot: apply snapshot failed");
                }
                Ok(false) => continue,
                Ok(true) => {
                    any_applied = true;
                    if row.state == "open" || row.state == "draft" {
                        any_open_applied = true;
                    }
                    if let Some(on_applied) = &self.on_applied {
                        on_applied(pr_id);
                    }
                }
            }
        }

        // Chase decision. Chase only while the snapshot is undecided AND we
        // still have an open PR row on this head. If nothing applied (head
        // advanced past this response, or the PR is gone), the webhook that
        // moved the head has already enqueued the fresh head, so we stop here.
        if any_applied && any_open_applied && !snap.decided() {
            self.schedule_chase(addr.clone());
        } else {
            self.lock_state().attempts.remove(addr);
        }
    }

    fn rate_limit_pause(&self, installation_id: i64) -> Duration {
        let mut st = self.lock_state();
        let Some(until) = st.rate_until.get(&installation_id).copied() else {
            return Duration::ZERO;
        };
        let now = (self.now)();
        if until <= now {
            st.rate_until.remove(&installation_id);
            return Duration::ZERO;
        }
        (until - now).to_std().unwrap_or(Duration::ZERO)
    }

    fn extend_rate_limit(&self, installation_id: i64, retry_after: Duration) {
        let until = (self.now)()
            + chrono::TimeDelta::from_std(retry_after)
                .unwrap_or_else(|_| chrono::TimeDelta::zero());
        let mut st = self.lock_state();
        let extends = st
            .rate_until
            .get(&installation_id)
            .is_none_or(|current| until > *current);
        if extends {
            st.rate_until.insert(installation_id, until);
        }
    }

    /// Returns a rate-limited address to the queue after delay without
    /// holding a worker. The active marker remains set while the timer owns
    /// the address, so duplicate triggers still coalesce into the scheduled
    /// fetch.
    fn defer_active(self: &Arc<Self>, addr: Address, delay: Duration) {
        let m = Arc::clone(self);
        self.spawn_task(async move {
            tokio::select! {
                _ = m.cancel.cancelled() => {}
                _ = tokio::time::sleep(delay) => {}
            }
            if m.cancel.is_cancelled() {
                m.release(addr);
                return;
            }
            if let Err(err) = m.queue_tx.try_send(addr.clone()) {
                m.release(addr);
                drop_marker_warn(err, "dropping rate-limited enqueue");
            }
        });
    }

    fn release(&self, addr: Address) {
        self.lock_state().release(&addr);
    }

    /// Releases an address after a worker completes it, or turns the single
    /// coalesced trailing edge into the next queued refresh without ever
    /// allowing two workers to own the same address concurrently.
    fn finish(&self, addr: Address) {
        let replay = {
            let mut st = self.lock_state();
            if !st.trailing.contains(&addr) {
                st.release(&addr);
                false
            } else {
                // active stays marked: duplicate triggers must keep coalescing
                // into this replayed refresh.
                st.trailing.remove(&addr);
                st.in_flight.remove(&addr);
                true
            }
        };
        if !replay {
            return;
        }
        // The worker just consumed one slot, so saturation is unlikely, but
        // keep the webhook path non-blocking and let the TTL sweep recover.
        if let Err(err) = self.queue_tx.try_send(addr.clone()) {
            self.lock_state().release(&addr);
            drop_marker_warn(err, "dropping trailing enqueue");
        }
    }

    /// Performs the head-SHA-guarded atomic batch replace for one PR row:
    /// guarded UPDATE of the snapshot columns, then a full DELETE + INSERT of
    /// the per-check rows — all in one transaction. Returns applied=false
    /// (and writes nothing) when the row's head has advanced past the
    /// snapshot's head.
    async fn apply_snapshot(&self, pr_id: Uuid, snap: &PrSnapshot) -> anyhow::Result<bool> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("ghsnapshot: apply snapshot without a database pool");
        };
        let mut tx = pool.begin().await?;
        let rollup = if snap.has_checks {
            text_or_null(&snap.rollup_state)
        } else {
            None
        };
        let n = dbq::update_git_hub_pr_snapshot(
            &mut *tx,
            text_or_null(&snap.mergeable),
            text_or_null(&snap.merge_state_status),
            rollup,
            &snap.head_sha,
            Some((self.now)()),
            pr_id,
        )
        .await?;
        if n == 0 {
            // Head advanced — discard the entire response, including the
            // per-check rows. Nothing is written (tx drops → rollback).
            return Ok(false);
        }
        dbq::delete_git_hub_pr_check_runs(&mut *tx, pr_id).await?;
        for (ordinal, c) in snap.contexts.iter().enumerate() {
            let ordinal = i32::try_from(ordinal).unwrap_or(i32::MAX);
            dbq::insert_git_hub_pr_check_run(
                &mut *tx,
                pr_id,
                &snap.head_sha,
                ordinal,
                &c.name,
                &c.status,
                c.is_status_context,
                text_or_null(&c.conclusion),
                text_or_null(&c.details_url),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Re-enqueues the address after the current backoff step. Bounded by
    /// [`MAX_CHASE_ATTEMPTS`] so an endlessly-pending CI or a wedged
    /// mergeability verdict can never spin forever.
    fn schedule_chase(self: &Arc<Self>, addr: Address) {
        let delay = {
            let mut st = self.lock_state();
            let attempt = st.attempts.get(&addr).copied().unwrap_or(0);
            if attempt >= MAX_CHASE_ATTEMPTS {
                st.attempts.remove(&addr);
                return;
            }
            st.attempts.insert(addr.clone(), attempt + 1);
            let idx = (attempt as usize).min(self.chase_backoff.len() - 1);
            self.chase_backoff[idx]
        };
        self.schedule_retry(addr, delay);
    }

    /// Re-enqueues the address after delay, unless the manager is shutting
    /// down.
    fn schedule_retry(self: &Arc<Self>, addr: Address, delay: Duration) {
        let m = Arc::clone(self);
        self.spawn_task(async move {
            tokio::select! {
                _ = m.cancel.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            if !m.cancel.is_cancelled() {
                m.enqueue(addr.installation_id, &addr.owner, &addr.repo, addr.number);
            }
        });
    }

    async fn sweep_loop(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.sweep_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // interval's first tick fires immediately; skip it
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => return,
                _ = ticker.tick() => self.sweep_once().await,
            }
        }
    }

    /// Enqueues a refresh for every open PR whose snapshot is both stale and
    /// undecided. Bounded by sweep_max_rows. This is the safety net for an
    /// undecided PR whose base branch changes without a pull_request webhook,
    /// and for any webhook that was dropped during a deploy.
    async fn sweep_once(&self) {
        let after = self.lock_state().sweep_after.clone();

        let Some(pool) = self.pool.as_ref() else {
            tracing::warn!("ghsnapshot: sweep skipped, no database pool");
            return;
        };
        let older_than = (self.now)()
            - chrono::TimeDelta::from_std(self.sweep_ttl)
                .unwrap_or_else(|_| chrono::TimeDelta::zero());
        let rows = match dbq::list_stale_undecided_git_hub_p_rs(
            pool,
            after.installation_id,
            &after.owner,
            &after.repo,
            after.number,
            self.sweep_max_rows,
            Some(older_than),
        )
        .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "ghsnapshot: sweep query failed");
                return;
            }
        };

        if let Some(last) = rows.last() {
            let mut st = self.lock_state();
            st.sweep_after = Address {
                installation_id: last.installation_id,
                owner: last.repo_owner.clone(),
                repo: last.repo_name.clone(),
                number: last.pr_number,
            };
        }
        for r in rows {
            self.enqueue(r.installation_id, r.repo_owner, r.repo_name, r.pr_number);
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn spawn_task(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::error!(%error, "ghsnapshot: background task stopped unexpectedly");
            }
        }
        if self
            .accepting_tasks
            .load(std::sync::atomic::Ordering::Acquire)
        {
            tasks.spawn(task);
        }
    }

    fn stop_accepting_tasks(&self) -> JoinSet<()> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.accepting_tasks
            .store(false, std::sync::atomic::Ordering::Release);
        std::mem::take(&mut *tasks)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

/// Production-owned root for workers, the TTL sweep, and every retry timer.
pub struct ManagerRuntime {
    manager: Arc<Manager>,
}

impl ManagerRuntime {
    pub async fn shutdown(self, timeout: Duration) -> ManagerShutdownOutcome {
        self.manager.cancel.cancel();
        let mut tasks = self.manager.stop_accepting_tasks();
        let mut panicked = false;
        let joined = tokio::time::timeout(timeout, async {
            while let Some(result) = tasks.join_next().await {
                if result.is_err() {
                    panicked = true;
                }
            }
        })
        .await;
        if joined.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            return ManagerShutdownOutcome::TimedOut;
        }
        if panicked {
            ManagerShutdownOutcome::Panicked
        } else {
            ManagerShutdownOutcome::Stopped
        }
    }
}

impl Drop for ManagerRuntime {
    fn drop(&mut self) {
        self.manager.cancel.cancel();
        let mut tasks = self.manager.stop_accepting_tasks();
        tasks.abort_all();
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn text_or_null(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Sleeps for d or until cancelled; returns false if cancelled.
async fn sleep_or_cancel(d: Duration, cancel: &tokio_util::sync::CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(d) => true,
    }
}

/// Saturated-queue tail shared by enqueue/finish/defer_active. `_err` (the
/// dropped address) is deliberately discarded: the caller already released
/// its markers, matching Go's warn-and-drop behavior.
fn drop_marker_warn(_err: mpsc::error::TrySendError<Address>, msg: &'static str) {
    tracing::warn!("ghsnapshot: refresh queue full, {msg}");
}

// ── tests ────────────────────────────────────────────────────────────────────
// Ports of refresh_test.go (unit) and refresh_db_test.go (live-Postgres,
// skipped when DATABASE_URL is unset).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DEFAULT_API_BASE;
    use crate::snapshot::CheckContext;

    fn generated_key() -> jsonwebtoken::EncodingKey {
        use rand::rngs::StdRng;
        use rand::SeedableRng;
        use rsa::pkcs8::EncodePrivateKey;
        let mut rng = StdRng::seed_from_u64(0xC0FFEE);
        let key = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("rsa generation");
        let pem = key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("pem encode");
        jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).expect("parse generated pem")
    }

    fn enabled_client() -> Client {
        Client::with_encoding_key(
            "1".to_string(),
            generated_key(),
            DEFAULT_API_BASE.to_string(),
            Box::new(std::time::SystemTime::now),
        )
        .expect("client construction")
    }

    fn addr(installation_id: i64, owner: &str, repo: &str, number: i32) -> Address {
        Address {
            installation_id,
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        }
    }

    fn fixed_clock(at: DateTime<Utc>) -> Box<dyn Fn() -> DateTime<Utc> + Send + Sync> {
        Box::new(move || at)
    }

    fn at_unix(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).expect("valid timestamp")
    }

    /// TestManagerDisabledNoOps: with no App key the manager touches nothing
    /// (clean-degradation guarantee).
    #[tokio::test]
    async fn manager_disabled_no_ops() {
        let m = Manager::new(None, None, None);
        assert!(!m.enabled(), "nil-client manager must be disabled");
        m.enqueue(1, "o", "r", 2);
        m.maybe_enqueue_on_view(1, "o", "r", 2, None);
        assert_eq!(m.queued_len().await, 0, "disabled manager must not enqueue");
        // Start must be a safe no-op (no workers, no panic).
        let m = Arc::new(m);
        assert!(m
            .start(tokio_util::sync::CancellationToken::new())
            .is_none());
    }

    #[tokio::test]
    async fn manager_runtime_is_owned_and_start_is_idempotent() {
        let m = Arc::new(Manager::new(Some(enabled_client()), None, None));
        let root = tokio_util::sync::CancellationToken::new();
        let runtime = m.start(root.child_token()).expect("enabled manager starts");
        assert!(m.start(root.child_token()).is_none());

        root.cancel();
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)).await,
            ManagerShutdownOutcome::Stopped
        );
    }

    /// TestEnqueueCoalesces: the same PR address enqueued repeatedly coalesces
    /// to one pending item; distinct addresses are not.
    #[tokio::test]
    async fn enqueue_coalesces() {
        let m = Manager::new(Some(enabled_client()), None, None);
        // Workers are NOT started, so items accumulate in the channel.
        m.enqueue(1, "o", "r", 7);
        m.enqueue(1, "o", "r", 7);
        m.enqueue(1, "o", "r", 7);
        assert_eq!(
            m.queued_len().await,
            1,
            "same address enqueued 3× must produce 1 queued item"
        );
        m.enqueue(1, "o", "r", 8); // different PR
        m.enqueue(1, "o", "other", 7); // different repo
        assert_eq!(m.queued_len().await, 3, "want 3 distinct queued items");
    }

    /// TestMaybeEnqueueOnViewRespectsTTL: a fresh snapshot is not refreshed on
    /// view; a stale or missing one is.
    #[tokio::test]
    async fn maybe_enqueue_on_view_respects_ttl() {
        let mut m = Manager::new(Some(enabled_client()), None, None);
        let now = at_unix(10_000);
        m.now = fixed_clock(now);

        m.maybe_enqueue_on_view(1, "o", "r", 1, Some(now - chrono::Duration::seconds(10))); // fresh (<60s)
        assert_eq!(
            m.queued_len().await,
            0,
            "fresh snapshot should not refresh on view"
        );

        m.maybe_enqueue_on_view(1, "o", "r", 2, Some(now - chrono::Duration::minutes(5))); // stale
        m.maybe_enqueue_on_view(1, "o", "r", 3, None); // never fetched
        assert_eq!(m.queued_len().await, 2, "stale/missing snapshots enqueued");
    }

    /// TestProcessRateLimitedSetsPause: a rate-limited fetch records a pause
    /// only for that installation and writes nothing.
    #[tokio::test]
    async fn process_rate_limited_sets_pause() {
        let mut m = Manager::new(Some(enabled_client()), None, None);
        let now = at_unix(20_000);
        m.now = fixed_clock(now);
        m.jitter = Box::new(|| Duration::ZERO);
        // Pre-cancelled token keeps the rescheduled retry from lingering.
        m.cancel.cancel();
        let fetch: FetchFn = Arc::new(|_client, _addr| {
            Box::pin(async {
                Err(anyhow::Error::new(RateLimitError {
                    retry_after: Duration::from_secs(90),
                }))
            })
        });
        m.set_fetch(fetch);
        // pool is None; the fetch errors before any DB access, proving the
        // rate-limit path never touches storage.
        let m = Arc::new(m);
        m.process(&addr(1, "o", "r", 1)).await;

        let want = now + chrono::Duration::seconds(90);
        assert_eq!(
            m.lock_state().rate_until.get(&1).copied(),
            Some(want),
            "rateUntil must be now+90s"
        );
    }

    #[tokio::test]
    async fn rate_limit_deadline_never_shortens() {
        let mut m = Manager::new(Some(enabled_client()), None, None);
        let now = at_unix(21_000);
        m.now = fixed_clock(now);

        m.extend_rate_limit(1, Duration::from_secs(90));
        m.extend_rate_limit(1, Duration::from_secs(30));
        assert_eq!(
            m.lock_state().rate_until.get(&1).copied(),
            Some(now + chrono::Duration::seconds(90)),
            "shorter Retry-After must not replace deadline"
        );

        m.extend_rate_limit(1, Duration::from_secs(120));
        assert_eq!(
            m.lock_state().rate_until.get(&1).copied(),
            Some(now + chrono::Duration::seconds(120)),
            "later Retry-After must be retained"
        );
    }

    #[tokio::test]
    async fn rate_limit_isolated_by_installation() {
        let mut m = Manager::new(Some(enabled_client()), None, None);
        let now = at_unix(22_000);
        m.now = fixed_clock(now);
        m.jitter = Box::new(|| Duration::ZERO);
        m.extend_rate_limit(1, Duration::from_secs(3600));

        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_for_fetch = Arc::clone(&called);
        let fetch: FetchFn = Arc::new(move |_client, _addr| {
            called_for_fetch.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Err(anyhow::anyhow!("stop after fetch")) })
        });
        m.set_fetch(fetch);
        let m = Arc::new(m);
        m.process(&addr(2, "o", "r", 1)).await;
        assert!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            "installation 1 rate limit blocked installation 2"
        );
        assert_eq!(
            m.rate_limit_pause(2),
            Duration::ZERO,
            "installation 2 unexpectedly paused"
        );
    }

    #[tokio::test]
    async fn persistent_rate_limit_returns_to_ttl_sweep() {
        let mut m = Manager::new(Some(enabled_client()), None, None);
        m.jitter = Box::new(|| Duration::ZERO);
        let fetch: FetchFn = Arc::new(|_client, _addr| {
            Box::pin(async {
                Err(anyhow::Error::new(RateLimitError {
                    retry_after: Duration::from_millis(1),
                }))
            })
        });
        m.set_fetch(fetch);

        let m = Arc::new(m);
        m.process(&addr(1, "o", "r", 1)).await;
        // No unbounded direct retry: ownership returns to the bounded
        // open/draft TTL sweep or a later external event.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            m.queued_len().await,
            0,
            "rate-limited fetch scheduled direct retries"
        );
    }

    /// TestScheduleChaseBounded: after maxChaseAttempts the address stops
    /// being rescheduled (chase window terminates).
    #[tokio::test]
    async fn schedule_chase_bounded() {
        let m = Arc::new(Manager::new(Some(enabled_client()), None, None));
        m.cancel.cancel(); // rescheduled retries are no-ops under a dead token
        let a = addr(1, "o", "r", 1);
        for i in 1..=MAX_CHASE_ATTEMPTS {
            m.schedule_chase(a.clone());
            assert_eq!(
                m.lock_state().attempts.get(&a).copied(),
                Some(i),
                "attempt {i} recorded incorrectly"
            );
        }
        // One more chase past the cap clears tracking and does not reschedule.
        m.schedule_chase(a.clone());
        assert!(
            !m.lock_state().attempts.contains_key(&a),
            "chase past the cap must stop and clear attempts"
        );
    }

    /// TestRateLimitedInstallationDoesNotOccupyWorkers: a full worker pool
    /// owned by the paused installation cannot starve another tenant.
    #[tokio::test(flavor = "multi_thread")]
    async fn rate_limited_installation_does_not_occupy_workers() {
        let mut m = Manager::new(Some(enabled_client()), None, None);
        m.concurrency = 12;
        m.sweep_interval = Duration::from_secs(3600);
        m.jitter = Box::new(|| Duration::ZERO);
        m.extend_rate_limit(1, Duration::from_secs(2));

        let limited_fetched = mpsc::channel::<i64>(16);
        let (limited_tx, mut limited_rx) = limited_fetched;
        let (other_tx, mut other_rx) = mpsc::channel::<i64>(16);
        let fetch: FetchFn = Arc::new(move |_client, a| {
            let tx = if a.installation_id == 1 {
                limited_tx.clone()
            } else {
                other_tx.clone()
            };
            Box::pin(async move {
                let _ = tx.send(a.installation_id).await;
                Err(anyhow::anyhow!("stop after fetch"))
            })
        });
        m.set_fetch(fetch);

        // Fill an entire worker pool with addresses from the paused
        // installation, then queue an unrelated tenant behind them.
        for number in 1..=(m.concurrency as i32) {
            m.enqueue(1, "o", "r", number);
        }
        m.enqueue(2, "o", "r", 1);

        let m = Arc::new(m);
        let _runtime = m
            .start(tokio_util::sync::CancellationToken::new())
            .expect("enabled manager starts");

        let got_other = tokio::time::timeout(Duration::from_millis(500), other_rx.recv()).await;
        assert!(
            got_other.is_ok(),
            "rate-limited installation occupied the global worker pool"
        );
        let leaked = tokio::time::timeout(Duration::from_millis(100), limited_rx.recv()).await;
        assert!(
            leaked.is_err(),
            "paused installation fetched before Retry-After"
        );
    }

    // ---- DB-backed tests (skipped without DATABASE_URL) ----

    async fn test_pool() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .ok()
    }

    struct SeededPr {
        id: Uuid,
        workspace_id: Uuid,
    }

    /// Seeds a minimal workspace + PR row; returns cleanup handles. The
    /// github_pull_request row predates the no-FK convention and carries a
    /// workspace FK, so a real workspace row is required.
    async fn seed_pr_at(
        pool: &PgPool,
        installation_id: i64,
        repo_name: &str,
        pr_number: i32,
        head_sha: &str,
    ) -> SeededPr {
        let slug = format!("ghsnap-{}", Uuid::now_v7().simple());
        let ws_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug, description, issue_prefix) VALUES ($1,$2,$3,$4) RETURNING id",
        )
        .bind("ghsnap test")
        .bind(&slug)
        .bind("ghsnap test workspace")
        .bind("GHS")
        .fetch_one(pool)
        .await
        .expect("seed workspace");

        let ts = at_unix(1_700_000_000);
        let pr = cordy_db::queries::github::upsert_git_hub_pull_request(
            pool,
            ws_id,
            installation_id,
            "o",
            repo_name,
            pr_number,
            "t",
            "open",
            "http://x",
            Some(ts),
            Some(ts),
            head_sha,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("seed PR")
        .expect("seed PR returned row");

        SeededPr {
            id: pr.id,
            workspace_id: ws_id,
        }
    }

    async fn cleanup_pr(pool: &PgPool, pr: &SeededPr) {
        let _ = sqlx::query("DELETE FROM github_pull_request_check_run WHERE pr_id=$1")
            .bind(pr.id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM github_pull_request WHERE id=$1")
            .bind(pr.id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM workspace WHERE id=$1")
            .bind(pr.workspace_id)
            .execute(pool)
            .await;
    }

    async fn check_run_count(pool: &PgPool, pr_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM github_pull_request_check_run WHERE pr_id=$1")
            .bind(pr_id)
            .fetch_one(pool)
            .await
            .expect("count check runs")
    }

    /// TestApplySnapshotHeadSHAGuard regression: a slow response for an old
    /// head must never overwrite a newer head's snapshot.
    #[tokio::test]
    async fn apply_snapshot_head_sha_guard() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let now = at_unix(1_700_000_100);
        let pr = seed_pr_at(&pool, 987_654, "r", 4242, "B").await;

        let mut m = Manager::new(None, Some(pool.clone()), None);
        m.now = fixed_clock(now);

        // 1. A response for head "A" while the row is at "B" → discarded.
        let applied = m
            .apply_snapshot(
                pr.id,
                &PrSnapshot {
                    head_sha: "A".into(),
                    mergeable: "CONFLICTING".into(),
                    merge_state_status: "DIRTY".into(),
                    ..Default::default()
                },
            )
            .await
            .expect("apply");
        assert!(!applied, "mismatched-head snapshot must be discarded");
        let got = cordy_db::queries::github_snapshot::get_git_hub_pull_request_by_id(&pool, pr.id)
            .await
            .expect("get pr");
        let got = got.expect("row exists");
        assert!(
            got.snapshot_head_sha.is_empty() && got.api_mergeable.is_none(),
            "discarded write leaked into row"
        );

        // 2. Matching head "B" → applied; columns + check runs written.
        let applied = m
            .apply_snapshot(
                pr.id,
                &PrSnapshot {
                    head_sha: "B".into(),
                    mergeable: "MERGEABLE".into(),
                    merge_state_status: "CLEAN".into(),
                    rollup_state: "FAILURE".into(),
                    has_checks: true,
                    contexts: vec![
                        CheckContext {
                            name: "backend".into(),
                            status: "completed".into(),
                            conclusion: "failure".into(),
                            details_url: String::new(),
                            is_status_context: false,
                        },
                        CheckContext {
                            name: "vercel".into(),
                            status: "completed".into(),
                            conclusion: "success".into(),
                            details_url: String::new(),
                            is_status_context: true,
                        },
                    ],
                },
            )
            .await
            .expect("apply");
        assert!(applied, "matching-head snapshot must apply");
        let got = cordy_db::queries::github_snapshot::get_git_hub_pull_request_by_id(&pool, pr.id)
            .await
            .expect("get pr")
            .expect("row exists");
        assert_eq!(got.snapshot_head_sha, "B");
        assert_eq!(got.api_mergeable.as_deref(), Some("MERGEABLE"));
        assert_eq!(got.checks_rollup_state.as_deref(), Some("FAILURE"));
        assert_eq!(check_run_count(&pool, pr.id).await, 2);

        // 3. Head advances to "C"; a late in-flight response for "B" is
        //    discarded and does NOT overwrite the stored snapshot.
        sqlx::query("UPDATE github_pull_request SET head_sha='C' WHERE id=$1")
            .bind(pr.id)
            .execute(&pool)
            .await
            .expect("advance head");
        let applied = m
            .apply_snapshot(
                pr.id,
                &PrSnapshot {
                    head_sha: "B".into(),
                    mergeable: "CONFLICTING".into(),
                    merge_state_status: "DIRTY".into(),
                    ..Default::default()
                },
            )
            .await
            .expect("apply");
        assert!(!applied, "late response for old head must be discarded");
        let got = cordy_db::queries::github_snapshot::get_git_hub_pull_request_by_id(&pool, pr.id)
            .await
            .expect("get pr")
            .expect("row exists");
        assert_eq!(got.snapshot_head_sha, "B");
        assert_eq!(got.api_mergeable.as_deref(), Some("MERGEABLE"));
        assert_eq!(
            check_run_count(&pool, pr.id).await,
            2,
            "check runs unchanged after stale late write"
        );

        cleanup_pr(&pool, &pr).await;
    }

    /// TestApplySnapshotReplacesRuns: each successful apply is an atomic batch
    /// replace, not an accumulation.
    #[tokio::test]
    async fn apply_snapshot_replaces_runs() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let now = at_unix(1_700_000_200);
        let pr = seed_pr_at(&pool, 987_654, "r", 4242, "H").await;

        let mut m = Manager::new(None, Some(pool.clone()), None);
        m.now = fixed_clock(now);

        let running = |name: &str| CheckContext {
            name: name.into(),
            status: "in_progress".into(),
            conclusion: String::new(),
            details_url: String::new(),
            is_status_context: false,
        };
        let three = PrSnapshot {
            head_sha: "H".into(),
            mergeable: "MERGEABLE".into(),
            merge_state_status: "CLEAN".into(),
            rollup_state: "PENDING".into(),
            has_checks: true,
            contexts: vec![running("a"), running("b"), running("c")],
        };
        m.apply_snapshot(pr.id, &three).await.expect("first apply");
        assert_eq!(check_run_count(&pool, pr.id).await, 3, "after first apply");

        let one = PrSnapshot {
            head_sha: "H".into(),
            mergeable: "MERGEABLE".into(),
            merge_state_status: "CLEAN".into(),
            rollup_state: "SUCCESS".into(),
            has_checks: true,
            contexts: vec![CheckContext {
                name: "a".into(),
                status: "completed".into(),
                conclusion: "success".into(),
                details_url: String::new(),
                is_status_context: false,
            }],
        };
        m.apply_snapshot(pr.id, &one).await.expect("replace");
        assert_eq!(
            check_run_count(&pool, pr.id).await,
            1,
            "old runs deleted on replace"
        );

        cleanup_pr(&pool, &pr).await;
    }

    /// TestInFlightOldHeadKeepsTrailingRefresh: while head A is fetching, a
    /// webhook advances the mirrored row to B and enqueues again. A is
    /// discarded by the head guard, but the coalesced trailing edge must
    /// still fetch and apply B immediately.
    #[tokio::test(flavor = "multi_thread")]
    async fn in_flight_old_head_keeps_trailing_refresh() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pr = seed_pr_at(&pool, 987_654, "r", 4242, "A").await;

        let (first_started_tx, mut first_started_rx) = mpsc::channel::<()>(1);
        let (release_first_tx, release_first_rx) = tokio::sync::oneshot::channel::<()>();
        let (second_fetched_tx, mut second_fetched_rx) = mpsc::channel::<()>(1);
        let (applied_tx, mut applied_rx) = mpsc::channel::<Uuid>(1);

        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        // One-shot release gate: only the first fetch blocks on it.
        let release_gate = Arc::new(tokio::sync::Mutex::new(Some(release_first_rx)));

        let snapshot_a = PrSnapshot {
            head_sha: "A".into(),
            mergeable: "MERGEABLE".into(),
            merge_state_status: "CLEAN".into(),
            rollup_state: "SUCCESS".into(),
            has_checks: true,
            contexts: vec![],
        };
        let snapshot_b = PrSnapshot {
            head_sha: "B".into(),
            mergeable: "CONFLICTING".into(),
            merge_state_status: "DIRTY".into(),
            rollup_state: "FAILURE".into(),
            has_checks: true,
            contexts: vec![CheckContext {
                name: "backend".into(),
                status: "completed".into(),
                conclusion: "failure".into(),
                details_url: String::new(),
                is_status_context: false,
            }],
        };

        let mut m = Manager::new(
            Some(enabled_client()),
            Some(pool.clone()),
            Some(Arc::new(move |id| {
                let _ = applied_tx.try_send(id);
            })),
        );
        m.concurrency = 2;
        m.sweep_interval = Duration::from_secs(3600);
        m.jitter = Box::new(|| Duration::ZERO);
        let fetch: FetchFn = {
            let calls = Arc::clone(&calls);
            let release_gate = Arc::clone(&release_gate);
            let second_fetched_tx = second_fetched_tx.clone();
            Arc::new(move |_client, _addr| {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let release_gate = Arc::clone(&release_gate);
                let second_fetched_tx = second_fetched_tx.clone();
                let first_started_tx = first_started_tx.clone();
                let snapshot_a = snapshot_a.clone();
                let snapshot_b = snapshot_b.clone();
                Box::pin(async move {
                    if n == 1 {
                        let _ = first_started_tx.send(()).await;
                        let rx = release_gate.lock().await.take();
                        if let Some(rx) = rx {
                            let _ = rx.await;
                        }
                        return Ok(snapshot_a);
                    }
                    let _ = second_fetched_tx.send(()).await;
                    Ok(snapshot_b)
                })
            })
        };
        m.set_fetch(fetch);

        let m = Arc::new(m);
        let _runtime = m
            .start(tokio_util::sync::CancellationToken::new())
            .expect("enabled manager starts");
        m.enqueue(987_654, "o", "r", 4242);
        tokio::time::timeout(Duration::from_secs(2), first_started_rx.recv())
            .await
            .expect("first head fetch did not start")
            .expect("first fetch signal");

        sqlx::query("UPDATE github_pull_request SET head_sha='B' WHERE id=$1")
            .bind(pr.id)
            .execute(&pool)
            .await
            .expect("advance head to B");
        m.enqueue(987_654, "o", "r", 4242);

        let raced = tokio::time::timeout(Duration::from_millis(20), second_fetched_rx.recv()).await;
        assert!(
            raced.is_err(),
            "second fetch started concurrently; single-PR in-flight guard failed"
        );
        let _ = release_first_tx.send(());

        tokio::time::timeout(Duration::from_secs(2), second_fetched_rx.recv())
            .await
            .expect("new-head trailing refresh was swallowed")
            .expect("second fetch signal");
        tokio::time::timeout(Duration::from_secs(2), applied_rx.recv())
            .await
            .expect("new-head snapshot was not applied")
            .expect("applied signal");

        let got = cordy_db::queries::github_snapshot::get_git_hub_pull_request_by_id(&pool, pr.id)
            .await
            .expect("get pr")
            .expect("row exists");
        assert_eq!(got.snapshot_head_sha, "B");
        assert_eq!(got.api_mergeable.as_deref(), Some("CONFLICTING"));
        assert_eq!(
            check_run_count(&pool, pr.id).await,
            1,
            "new-head check runs"
        );

        cleanup_pr(&pool, &pr).await;
    }

    /// TestListStaleUndecidedGitHubPRsExcludesDecidedAndRotatesCursor: the
    /// periodic sweep skips decided snapshots and its cursor rotation cannot
    /// be pinned by a perpetually failing first address.
    #[tokio::test]
    async fn list_stale_undecided_excludes_decided_and_rotates_cursor() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let now = at_unix(1_700_010_000);

        let settled = seed_pr_at(&pool, 111, "settled", 1, "S").await;
        let oldest = seed_pr_at(&pool, 111, "oldest", 2, "O").await;
        let running = seed_pr_at(&pool, 222, "running", 3, "R").await;
        let newer = seed_pr_at(&pool, 222, "newer", 4, "N").await;

        async fn set_snapshot(
            pool: &PgPool,
            now: DateTime<Utc>,
            pr_id: Uuid,
            age_min: i64,
            mergeable: &str,
            rollup: &str,
        ) {
            sqlx::query(
                r#"UPDATE github_pull_request
                   SET snapshot_head_sha=head_sha, snapshot_fetched_at=$2,
                       api_mergeable=$3, checks_rollup_state=$4
                   WHERE id=$1"#,
            )
            .bind(pr_id)
            .bind(now - chrono::Duration::minutes(age_min))
            .bind(mergeable)
            .bind(rollup)
            .execute(pool)
            .await
            .expect("set snapshot");
        }
        set_snapshot(&pool, now, settled.id, 60, "MERGEABLE", "SUCCESS").await; // decided
        set_snapshot(&pool, now, oldest.id, 40, "UNKNOWN", "PENDING").await;
        set_snapshot(&pool, now, running.id, 30, "MERGEABLE", "SUCCESS").await;
        set_snapshot(&pool, now, newer.id, 20, "MERGEABLE", "PENDING").await;
        sqlx::query(
            r#"INSERT INTO github_pull_request_check_run
                   (pr_id, head_sha, ordinal, name, status, is_status_context)
               VALUES ($1, 'R', 0, 'backend', 'in_progress', false)"#,
        )
        .bind(running.id)
        .execute(&pool)
        .await
        .expect("seed running check run");

        let older_than = now - chrono::Duration::minutes(10);
        let rows =
            dbq::list_stale_undecided_git_hub_p_rs(&pool, 0, "", "", 0, 10, Some(older_than))
                .await
                .expect("sweep query");
        let repos: Vec<&str> = rows.iter().map(|r| r.repo_name.as_str()).collect();
        assert!(
            !repos.contains(&"settled"),
            "decided snapshot remained in the periodic TTL sweep"
        );
        for repo in ["oldest", "running", "newer"] {
            assert!(
                repos.contains(&repo),
                "undecided repo {repo} missing from TTL sweep: {repos:?}"
            );
        }

        let first =
            dbq::list_stale_undecided_git_hub_p_rs(&pool, 0, "", "", 0, 1, Some(older_than))
                .await
                .expect("first bounded sweep");
        assert_eq!(first.len(), 1, "bounded sweep returns one row");
        assert_eq!(
            first[0].repo_name, "oldest",
            "first bounded sweep starts at first address"
        );

        // Advance from the last returned address without changing its stale
        // data. Even a perpetually failing first address cannot pin LIMIT.
        let second = dbq::list_stale_undecided_git_hub_p_rs(
            &pool,
            first[0].installation_id,
            &first[0].repo_owner,
            &first[0].repo_name,
            first[0].pr_number,
            1,
            Some(older_than),
        )
        .await
        .expect("second bounded sweep");
        assert_eq!(second.len(), 1, "second bounded sweep returns one row");
        assert_eq!(
            second[0].repo_name, "newer",
            "cursor advanced past first address"
        );

        for pr in [&settled, &oldest, &running, &newer] {
            cleanup_pr(&pool, pr).await;
        }
    }
}
