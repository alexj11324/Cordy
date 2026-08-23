//! Port of `server/internal/daemon/auto_update.go` (lines 1–433).
//!
//! Everything that can end in "restart this daemon into a different cordy
//! binary": the GitHub release poll ([`try_auto_update`]) and the out-of-band
//! on-disk binary reload probe ([`try_self_reload`]), both driven by
//! [`auto_update_loop`] on one task so they cannot race each other into a
//! restart.
//!
//! Deviations from Go:
//! - Daemon state (cfg, updating flag, activeTasks, claim barrier, restart
//!   plumbing) is reached through the [`AutoUpdateHost`] seam instead of
//!   `*Daemon` — manager.rs is another lane. The claim-barrier mechanics
//!   themselves are ported here as [`ClaimBarrier`] (daemon.go:4524–4570) so
//!   the poller-side test ports 1:1; the Daemon core embeds it at integration.
//! - The `fetchLatestRelease` / `isReleaseVersion` / `isNewerVersion`
//!   indirection vars become trait methods plus free functions; the pure
//!   version helpers are ported verbatim from internal/cli/update.go
//!   (S9-integration: that package belongs to another lane).
//! - `log/slog` → `tracing` with identical message text.

// S9-integration: dead_code until Daemon core wires this.
#![allow(dead_code)]

use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;

use crate::config::DEFAULT_AUTO_UPDATE_CHECK_INTERVAL;
use crate::repocache::{CancelCause, Ctx};

/// autoUpdateInitialDelay (auto_update.go:63–69).
pub(crate) const AUTO_UPDATE_INITIAL_DELAY: Duration = Duration::from_secs(2 * 60);

/// selfReloadCheckInterval (auto_update.go:71–86).
pub(crate) const SELF_RELOAD_CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// selfReloadProbeTimeout (auto_update.go:88–91).
pub(crate) const SELF_RELOAD_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// S9-integration stand-in for `cli.GitHubRelease`: only TagName is consumed
/// by the daemon's update poller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubRelease {
    pub tag_name: String,
}

/// Failure of a run-update invocation, carrying the partial command output so
/// the warn log can include it exactly like Go's `"output", output` pair.
#[derive(Debug)]
pub(crate) struct RunUpdateFailure {
    pub source: anyhow::Error,
    pub output: String,
}

// ---------------------------------------------------------------------------
// Pure version helpers (S9-integration: ported from internal/cli/update.go).
// ---------------------------------------------------------------------------

/// `parseReleaseVersion` (internal/cli/update.go): extracts the three numeric
/// components of v, or None when v is missing, malformed, or carries any
/// non-numeric tail (a dev-describe suffix, a 4th component, a pre-release
/// tag). The strict shape is intentional: this is the only parser used by
/// [`is_newer_version`], and the auto-update loop must never silently
/// downgrade a developer build to a public release just because the
/// dev-describe patch happened to look numeric after trimming.
fn parse_release_version(v: &str) -> Option<[u64; 3]> {
    let s = v.trim().strip_prefix('v').unwrap_or(v.trim());
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut out = [0u64; 3];
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out[i] = p.parse().ok()?;
    }
    Some(out)
}

/// `IsReleaseVersion` (internal/cli/update.go:47–68): reports whether v looks
/// like a tagged release version rather than a dev build.
pub(crate) fn is_release_version(v: &str) -> bool {
    let s = v.trim().strip_prefix('v').unwrap_or(v.trim());
    if s.is_empty() {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `IsNewerVersion` (internal/cli/update.go:73–89): reports whether latest is
/// strictly newer than current. Both may carry an optional "v" prefix;
/// non-numeric tails are ignored. Returns false if either side cannot be
/// parsed — the caller treats that as "stay on current".
pub(crate) fn is_newer_version(latest: &str, current: &str) -> bool {
    let Some(l) = parse_release_version(latest) else {
        return false;
    };
    let Some(c) = parse_release_version(current) else {
        return false;
    };
    for i in 0..3 {
        if l[i] != c[i] {
            return l[i] > c[i];
        }
    }
    false
}

/// `ParseSelfVersion` (auto_update.go:40–61): pulls the version out of
/// `cordy --version` output, whose first line is rendered by cmd/cordy's
/// version template. Anything that doesn't match the template shape is
/// returned trimmed, which compares unequal and is therefore reported rather
/// than silently ignored.
pub(crate) fn parse_self_version(raw: &str) -> String {
    let line = raw.split('\n').next().unwrap_or("").trim();
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() >= 2 && fields[0] == "cordy" {
        return fields[1].to_string();
    }
    line.to_string()
}

/// Default `detectSelfVersion` body (auto_update.go:25–38): runs
/// `<path> --version` and parses the output.
///
/// Deviation vs Go: Go's `WaitDelay` force-closes pipes 2 s after kill so an
/// orphaned descendant cannot park the caller on stdout; here the child is
/// spawned with `kill_on_drop(true)` and the caller's probe timeout bounds the
/// whole wait, which closes the same park-forever window.
pub(crate) async fn detect_self_version_default(
    ctx: &Ctx,
    path: &str,
) -> anyhow::Result<String> {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    let mut child = Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context(format!("run {path} --version"))?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err(anyhow::anyhow!("run {path} --version: stdout unavailable"));
    };
    let mut out = Vec::new();
    tokio::select! {
        _ = ctx.cancelled() => return Err(anyhow::anyhow!("{}", ctx.cause())),
        read = stdout.read_to_end(&mut out) => {
            read.context(format!("run {path} --version"))?;
        }
    }
    let status = child.wait().await.context(format!("run {path} --version"))?;
    if !status.success() {
        return Err(anyhow::anyhow!("run {path} --version: exit status {:?}", status.code()));
    }
    Ok(parse_self_version(&String::from_utf8_lossy(&out)))
}

// ---------------------------------------------------------------------------
// Claim barrier (daemon.go:4524–4570, S9-integration: embedded by Daemon core).
// ---------------------------------------------------------------------------

/// Mirrors Daemon's `claimMu` + `pauseClaims` + `claimsInFlight` triple.
#[derive(Debug, Default)]
pub(crate) struct ClaimBarrier {
    inner: Mutex<ClaimBarrierInner>,
}

#[derive(Debug, Default)]
struct ClaimBarrierInner {
    pause_claims: bool,
    claims_in_flight: i64,
}

impl ClaimBarrier {
    /// `tryEnterClaim` (daemon.go:4524–4532).
    pub(crate) fn try_enter_claim(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.pause_claims {
            return false;
        }
        inner.claims_in_flight += 1;
        true
    }

    /// `exitClaim` (daemon.go:4535–4541).
    pub(crate) fn exit_claim(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.claims_in_flight -= 1;
    }

    /// `trySetClaimBarrier` (daemon.go:4548–4564): atomically pauses new
    /// ClaimTask calls if the daemon is fully idle. Refuses when the barrier
    /// is already held — without this two holders would both believe they own
    /// it and whichever finished first would release it out from under the
    /// other.
    pub(crate) fn try_set_claim_barrier(&self, active_tasks: i64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.pause_claims || inner.claims_in_flight > 0 || active_tasks > 0 {
            return false;
        }
        inner.pause_claims = true;
        true
    }

    /// `releaseClaimBarrier` (daemon.go:4566–4570): failure paths only.
    pub(crate) fn release_claim_barrier(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pause_claims = false;
    }

    fn pause_claims(&self) -> bool {
        self.inner.lock().unwrap().pause_claims
    }

    fn claims_in_flight(&self) -> i64 {
        self.inner.lock().unwrap().claims_in_flight
    }
}

// ---------------------------------------------------------------------------
// Host seam (auto_update.go reaches through *Daemon).
// ---------------------------------------------------------------------------

/// S9-integration seam for the Daemon pieces the auto-update loop touches.
/// The Daemon core implements this over its own state at integration time.
pub(crate) trait AutoUpdateHost: Sync {
    /// `d.cfg`.
    fn cfg(&self) -> &crate::config::Config;

    /// `d.updating.Load()`.
    fn updating_load(&self) -> bool;

    /// `d.updating.CompareAndSwap(false, true)` — acquire update ownership.
    fn updating_cas_acquire(&self) -> bool;

    /// `d.updating.Store(v)`.
    fn updating_store(&self, v: bool);

    /// `d.activeTasks.Load()`.
    fn active_tasks(&self) -> i64;

    /// `d.trySetClaimBarrier()` reads activeTasks itself in Go; the Rust seam
    /// passes it explicitly from [`active_tasks`](Self::active_tasks).
    fn try_set_claim_barrier(&self) -> bool;

    /// `d.releaseClaimBarrier()`.
    fn release_claim_barrier(&self);

    /// `d.RestartBinary()`.
    fn restart_binary(&self) -> String;

    /// `d.setReloadPending(reason)` / `d.clearReloadPending()`.
    fn set_reload_pending_reason(&self, reason: Option<String>);

    /// `d.triggerRestart()`.
    fn trigger_restart(&self) -> bool;

    /// `d.restartTargetBinary()`.
    fn restart_target_binary(&self) -> anyhow::Result<String>;

    /// `d.runUpdateFn(target)` → (output, error).
    fn run_update(&self, target_version: &str) -> Result<String, RunUpdateFailure>;

    /// `fetchLatestRelease` indirection var.
    fn fetch_latest_release(&self) -> anyhow::Result<Option<GitHubRelease>>;

    /// `detectSelfVersion` indirection var.
    fn detect_self_version<'a>(
        &'a self,
        ctx: &'a Ctx,
        path: &'a str,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<String>>;
}

// ---------------------------------------------------------------------------
// Loop bodies (auto_update.go:116–411).
// ---------------------------------------------------------------------------

async fn sleep_with_context(ctx: &Ctx, d: Duration) -> Result<(), CancelCause> {
    tokio::select! {
        _ = ctx.cancelled() => Err(ctx.cause()),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

/// `autoUpdateLoop` (auto_update.go:116–189).
pub(crate) async fn auto_update_loop(ctx: &Ctx, host: &dyn AutoUpdateHost) {
    if host.cfg().launched_by == "desktop" {
        tracing::info!("auto-update: skipped (managed by Desktop)");
        return;
    }

    let mut pull_enabled = host.cfg().auto_update_enabled;
    if !pull_enabled {
        tracing::info!("auto-update: disabled");
    } else if !is_release_version(&host.cfg().cli_version) {
        tracing::info!(
            version = %host.cfg().cli_version,
            "auto-update: skipped (not a release build)"
        );
        pull_enabled = false;
    }
    let reload_enabled = host.cfg().auto_reload_enabled;
    if !reload_enabled {
        tracing::info!("auto-reload: disabled");
    }
    if !pull_enabled && !reload_enabled {
        return;
    }

    let mut pull_interval = host.cfg().auto_update_check_interval;
    if pull_interval.is_zero() {
        pull_interval = DEFAULT_AUTO_UPDATE_CHECK_INTERVAL;
    }
    // Log what each half will do before the startup delay, so a user reading
    // daemon.log right after `daemon start` sees it immediately.
    if pull_enabled {
        tracing::info!(
            interval = ?pull_interval,
            current = %host.cfg().cli_version,
            "auto-update: started"
        );
    }
    if reload_enabled {
        tracing::info!(
            interval = ?SELF_RELOAD_CHECK_INTERVAL,
            current = %host.cfg().cli_version,
            "auto-reload: watching the cordy binary on disk"
        );
    }

    if sleep_with_context(ctx, AUTO_UPDATE_INITIAL_DELAY).await.is_err() {
        return;
    }
    if pull_enabled {
        try_auto_update(ctx, host).await;
    }
    if reload_enabled {
        try_self_reload(ctx, host).await;
    }

    // Tickers start only now, after the first check (auto_update.go:162–177):
    // sleep-until deadlines instead of channel tickers so no buffered tick can
    // fire a second check immediately after the first. A disabled half stays
    // None, which never fires.
    let mut next_pull = pull_enabled.then(|| tokio::time::Instant::now() + pull_interval);
    let mut next_reload =
        reload_enabled.then(|| tokio::time::Instant::now() + SELF_RELOAD_CHECK_INTERVAL);

    loop {
        tokio::select! {
            _ = ctx.cancelled() => return,
            _ = async {
                match next_pull.as_mut() {
                    Some(deadline) => tokio::time::sleep_until(*deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                next_pull = Some(tokio::time::Instant::now() + pull_interval);
                try_auto_update(ctx, host).await;
            }
            _ = async {
                match next_reload.as_mut() {
                    Some(deadline) => tokio::time::sleep_until(*deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                next_reload = Some(tokio::time::Instant::now() + SELF_RELOAD_CHECK_INTERVAL);
                try_self_reload(ctx, host).await;
            }
        }
    }
}

/// `tryAutoUpdate` (auto_update.go:191–291): one check-and-maybe-upgrade
/// cycle. Never returns an error — a check that fails today is retried at the
/// next tick.
pub(crate) async fn try_auto_update(ctx: &Ctx, host: &dyn AutoUpdateHost) {
    if ctx.err().is_some() {
        return;
    }
    // Don't race the server-triggered update path.
    if host.updating_load() {
        tracing::debug!("auto-update: skip — update already in progress");
        return;
    }
    // Cheap pre-fetch idle check: no point paying the GitHub call when we
    // already know we are going to defer.
    let running = host.active_tasks();
    if running > 0 {
        tracing::debug!(active = running, "auto-update: skip — tasks running");
        return;
    }

    let release = match host.fetch_latest_release() {
        Ok(release) => release,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "auto-update: fetch latest release failed — will retry"
            );
            return;
        }
    };
    let Some(release) = release.filter(|r| !r.tag_name.is_empty()) else {
        return;
    };
    if !is_newer_version(&release.tag_name, &host.cfg().cli_version) {
        return;
    }

    // CAS the updating flag so a concurrent server-triggered handleUpdate
    // dropped onto a heartbeat tick can't double-fire.
    if !host.updating_cas_acquire() {
        tracing::debug!("auto-update: skip — update already in progress (raced)");
        return;
    }

    // Strict barrier between the cheap pre-fetch idle check and now. Every
    // exit below releases both the barrier and the flag except the restart
    // kick, where process exit is imminent (auto_update.go:284–290).
    if !host.try_set_claim_barrier() {
        tracing::info!("auto-update: deferring — task or claim in flight at barrier check");
        host.updating_store(false);
        return;
    }

    tracing::info!(
        current = %host.cfg().cli_version,
        target = %release.tag_name,
        "auto-update: newer release available, upgrading"
    );

    let output = match host.run_update(&release.tag_name) {
        Ok(output) => output,
        Err(failure) => {
            tracing::warn!(
                error = %failure.source,
                output = %failure.output,
                "auto-update: upgrade failed — will retry"
            );
            host.release_claim_barrier();
            host.updating_store(false);
            return;
        }
    };

    tracing::info!(
        target = %release.tag_name,
        output = %output,
        "auto-update: upgrade completed, restarting"
    );
    if !host.trigger_restart() {
        // The upgrade landed but the handoff target could not be resolved, so
        // no process exit is coming. Fall through to both restores: holding
        // the updating flag and the claim barrier for a restart that will
        // never happen stops this daemon from claiming any task, forever.
        tracing::error!(
            "auto-update: upgrade completed but restart could not be scheduled — resuming claims"
        );
        host.release_claim_barrier();
        host.updating_store(false);
        return;
    }
    // Process exit is imminent; leave both held (auto_update.go:284–290).
}

/// `trySelfReload` (auto_update.go:293–411): restarts the daemon when the
/// cordy binary on disk no longer reports the version compiled into this
/// process. A failed probe is never treated as a version change.
pub(crate) async fn try_self_reload(ctx: &Ctx, host: &dyn AutoUpdateHost) {
    if ctx.err().is_some() {
        return;
    }
    if !host.restart_binary().is_empty() {
        return;
    }
    // Acquire update ownership rather than sampling it (auto_update.go:333–348).
    if !host.updating_cas_acquire() {
        tracing::debug!("auto-reload: skip — update already in progress");
        return;
    }

    // Probe the binary the restart would actually exec, not os.Executable().
    let target = match host.restart_target_binary() {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "auto-reload: could not resolve own executable — will retry"
            );
            host.updating_store(false);
            return;
        }
    };
    let probe_ctx = ctx.child();
    let on_disk = match tokio::time::timeout(
        SELF_RELOAD_PROBE_TIMEOUT,
        host.detect_self_version(&probe_ctx, &target),
    )
    .await
    {
        Ok(Ok(on_disk)) => on_disk,
        Ok(Err(err)) => {
            tracing::warn!(
                binary = %target,
                error = %err,
                "auto-reload: version probe failed — will retry, not treating it as a change"
            );
            host.updating_store(false);
            return;
        }
        Err(_) => {
            tracing::warn!(
                binary = %target,
                error = "context deadline exceeded",
                "auto-reload: version probe failed — will retry, not treating it as a change"
            );
            host.updating_store(false);
            return;
        }
    };
    // Either side blank makes the comparison meaningless (auto_update.go:372–381).
    if on_disk.is_empty() || host.cfg().cli_version.is_empty() {
        tracing::warn!(
            binary = %target,
            on_disk = %on_disk,
            running = %host.cfg().cli_version,
            "auto-reload: version unavailable — will retry, not treating it as a change"
        );
        host.updating_store(false);
        return;
    }
    if on_disk == host.cfg().cli_version {
        host.set_reload_pending_reason(None);
        host.updating_store(false);
        return;
    }

    let reason = format!(
        "cordy binary on disk reports {}, running {}",
        on_disk, host.cfg().cli_version
    );
    host.set_reload_pending_reason(Some(reason.clone()));

    if !host.try_set_claim_barrier() {
        tracing::info!(
            reason = %reason,
            "auto-reload: deferring — task or claim in flight at barrier check"
        );
        host.updating_store(false);
        return;
    }

    tracing::info!(
        reason = %reason,
        binary = %target,
        "auto-reload: restarting into the binary on disk"
    );
    if !host.trigger_restart() {
        // Resolution failed at the handoff. Both restores run (auto_update.go:402–408).
        host.release_claim_barrier();
        host.updating_store(false);
    }
    // On success both stay held: process exit is imminent.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

    /// newAutoUpdateTestDaemon (auto_update_test.go:22–45): the Daemon pieces
    /// try_auto_update touches, plus a restart-call counter.
    struct TestHost {
        cfg: crate::config::Config,
        updating: AtomicBool,
        active_tasks: AtomicI64,
        barrier: ClaimBarrier,
        restart_calls: AtomicI32,
        restart_binary: Mutex<String>,
        reload_pending: Mutex<Option<String>>,
        release: Mutex<Option<Result<Option<GitHubRelease>, anyhow::Error>>>,
        run_update: Mutex<Option<Box<dyn Fn(&str) -> (String, Result<(), anyhow::Error>) + Send>>>,
    }

    impl TestHost {
        fn new(current_version: &str) -> Self {
            Self {
                cfg: crate::config::Config {
                    cli_version: current_version.into(),
                    auto_update_enabled: true,
                    ..Default::default()
                },
                updating: AtomicBool::new(false),
                active_tasks: AtomicI64::new(0),
                barrier: ClaimBarrier::default(),
                restart_calls: AtomicI32::new(0),
                restart_binary: Mutex::new(String::new()),
                reload_pending: Mutex::new(None),
                release: Mutex::new(None),
                run_update: Mutex::new(None),
            }
        }

        fn with_release(&self, tag: &str) {
            *self.release.lock().unwrap() = Some(Ok(Some(GitHubRelease {
                tag_name: tag.into(),
            })));
        }

        fn with_run_update(&self, f: impl Fn(&str) -> (String, Result<(), anyhow::Error>) + Send + 'static) {
            *self.run_update.lock().unwrap() = Some(Box::new(f));
        }
    }

    impl AutoUpdateHost for TestHost {
        fn cfg(&self) -> &crate::config::Config {
            &self.cfg
        }

        fn updating_load(&self) -> bool {
            self.updating.load(Ordering::SeqCst)
        }

        fn updating_cas_acquire(&self) -> bool {
            self.updating
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }

        fn updating_store(&self, v: bool) {
            self.updating.store(v, Ordering::SeqCst);
        }

        fn active_tasks(&self) -> i64 {
            self.active_tasks.load(Ordering::SeqCst)
        }

        fn try_set_claim_barrier(&self) -> bool {
            self.barrier.try_set_claim_barrier(self.active_tasks())
        }

        fn release_claim_barrier(&self) {
            self.barrier.release_claim_barrier();
        }

        fn restart_binary(&self) -> String {
            self.restart_binary.lock().unwrap().clone()
        }

        fn set_reload_pending_reason(&self, reason: Option<String>) {
            *self.reload_pending.lock().unwrap() = reason;
        }

        fn trigger_restart(&self) -> bool {
            self.restart_calls.fetch_add(1, Ordering::SeqCst);
            true
        }

        fn restart_target_binary(&self) -> anyhow::Result<String> {
            Ok("/usr/local/bin/cordy".into())
        }

        fn run_update(&self, target_version: &str) -> Result<String, RunUpdateFailure> {
            let f = self.run_update.lock().unwrap();
            match f.as_ref() {
                Some(f) => {
                    let (output, result) = f(target_version);
                    result.map(|()| output).map_err(|source| RunUpdateFailure { source, output })
                }
                // Mirrors the Go helper's t.Fatalf on unexpected calls.
                None => panic!("runUpdateFn called unexpectedly"),
            }
        }

        fn fetch_latest_release(&self) -> anyhow::Result<Option<GitHubRelease>> {
            match self.release.lock().unwrap().take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        fn detect_self_version<'a>(
            &'a self,
            _ctx: &'a Ctx,
            _path: &'a str,
        ) -> futures_util::future::BoxFuture<'a, anyhow::Result<String>> {
            // Mirrors the Go helper's absence of a stub: reload tests are not
            // in scope for this host, so an unexpected probe panics.
            Box::pin(async {
                panic!("detectSelfVersion called unexpectedly")
            })
        }
    }

    #[tokio::test]
    async fn try_auto_update_skips_when_updating() {
        let host = TestHost::new("v0.1.13");
        host.updating.store(true, Ordering::SeqCst);
        host.with_release("v0.1.14");

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(
            host.restart_calls.load(Ordering::SeqCst),
            0,
            "triggerRestart called while another update was in progress"
        );
    }

    #[tokio::test]
    async fn try_auto_update_skips_when_tasks_running() {
        let host = TestHost::new("v0.1.13");
        host.active_tasks.store(1, Ordering::SeqCst);
        host.with_release("v0.1.14");

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(
            host.restart_calls.load(Ordering::SeqCst),
            0,
            "triggerRestart fired with active tasks; auto-update must defer"
        );
        assert!(
            !host.updating.load(Ordering::SeqCst),
            "updating flag should not have been claimed while tasks were running"
        );
    }

    /// TestTryAutoUpdate_DefersWhenClaimInFlightAtBarrier: cheap pre-fetch
    /// idle check passes, then during the release fetch a poller claims —
    /// trySetClaimBarrier must observe that and defer.
    #[tokio::test]
    async fn try_auto_update_defers_when_claim_in_flight_at_barrier() {
        let host = TestHost::new("v0.1.13");
        host.with_release("v0.1.14");
        assert!(host.barrier.try_enter_claim(), "poller claim should succeed");

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(
            host.restart_calls.load(Ordering::SeqCst),
            0,
            "triggerRestart fired despite a claim being in flight at the barrier"
        );
        assert!(
            !host.updating.load(Ordering::SeqCst),
            "updating flag must be released after a deferred upgrade so the next tick can retry"
        );
        assert!(
            !host.barrier.pause_claims(),
            "pauseClaims must be cleared after a deferred upgrade"
        );
    }

    /// TestTryAutoUpdate_HoldsBarrierAcrossRestart.
    #[tokio::test]
    async fn try_auto_update_holds_barrier_across_restart() {
        let host = TestHost::new("v0.1.13");
        host.with_release("v0.1.14");
        host.with_run_update(|_| ("upgraded".into(), Ok(())));

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(
            host.restart_calls.load(Ordering::SeqCst),
            1,
            "triggerRestart fired more than once"
        );
        assert!(
            host.barrier.pause_claims(),
            "pauseClaims must remain set across the restart kick; got cleared"
        );
        assert!(
            host.updating.load(Ordering::SeqCst),
            "updating flag should remain set across the restart kick; got cleared"
        );
    }

    /// TestTryAutoUpdate_ReleasesBarrierOnUpgradeFailure.
    #[tokio::test]
    async fn try_auto_update_releases_barrier_on_upgrade_failure() {
        let host = TestHost::new("v0.1.13");
        host.with_release("v0.1.14");
        host.with_run_update(|_| {
            (
                "brew network error".into(),
                Err(anyhow::anyhow!("brew upgrade failed")),
            )
        });

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
        assert!(
            !host.barrier.pause_claims(),
            "pauseClaims must be cleared after a failed upgrade so pollers resume claiming"
        );
    }

    /// TestTryEnterClaim_RespectsBarrier.
    #[test]
    fn try_enter_claim_respects_barrier() {
        let barrier = ClaimBarrier::default();

        assert!(barrier.try_enter_claim(), "tryEnterClaim should succeed when barrier is unset");
        barrier.exit_claim();
        assert_eq!(barrier.claims_in_flight(), 0, "claimsInFlight not balanced");

        assert!(barrier.try_set_claim_barrier(0), "trySetClaimBarrier should succeed when idle");
        assert!(!barrier.try_enter_claim(), "tryEnterClaim must refuse while barrier is held");
        barrier.release_claim_barrier();
        assert!(barrier.try_enter_claim(), "tryEnterClaim should succeed after barrier release");
        barrier.exit_claim();
    }

    #[tokio::test]
    async fn try_auto_update_skips_when_fetch_fails() {
        let host = TestHost::new("v0.1.13");
        *host.release.lock().unwrap() = Some(Err(anyhow::anyhow!("network down")));

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn try_auto_update_skips_when_not_newer() {
        let host = TestHost::new("v0.1.13");
        host.with_release("v0.1.13");

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(
            host.restart_calls.load(Ordering::SeqCst),
            0,
            "triggerRestart fired even though latest == current"
        );
    }

    #[tokio::test]
    async fn try_auto_update_runs_upgrade_and_restarts_on_newer() {
        let host = TestHost::new("v0.1.13");
        host.with_release("v0.1.14");
        let upgraded_to = Mutex::new(String::new());
        host.with_run_update(|target| {
            *upgraded_to.lock().unwrap() = target.to_string();
            ("upgraded".into(), Ok(()))
        });

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(upgraded_to.lock().unwrap().as_str(), "v0.1.14");
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 1);
        assert!(
            host.updating.load(Ordering::SeqCst),
            "updating flag should remain set across the restart kick; got cleared"
        );
    }

    #[tokio::test]
    async fn try_auto_update_does_not_restart_on_upgrade_failure() {
        let host = TestHost::new("v0.1.13");
        host.with_release("v0.1.14");
        host.with_run_update(|_| {
            (
                "brew: network error".into(),
                Err(anyhow::anyhow!("brew upgrade failed")),
            )
        });

        try_auto_update(&Ctx::new(), &host).await;

        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
        assert!(
            !host.updating.load(Ordering::SeqCst),
            "updating flag must be released after a failed upgrade so the next tick can retry"
        );
    }

    /// TestAutoUpdateLoop_EarlyExits (auto_update_test.go:283–330).
    #[tokio::test]
    async fn auto_update_loop_early_exits() {
        let cases: Vec<(&str, crate::config::Config)> = vec![
            (
                "disabled by config",
                crate::config::Config {
                    auto_update_enabled: false,
                    cli_version: "v0.1.13".into(),
                    ..Default::default()
                },
            ),
            (
                "managed by desktop",
                crate::config::Config {
                    auto_update_enabled: true,
                    cli_version: "v0.1.13".into(),
                    launched_by: "desktop".into(),
                    ..Default::default()
                },
            ),
            (
                "dev build",
                crate::config::Config {
                    auto_update_enabled: true,
                    cli_version: "v0.1.13-235-gabcdef0".into(),
                    ..Default::default()
                },
            ),
        ];
        for (name, cfg) in cases {
            let mut host = TestHost::new("");
            host.cfg = cfg;
            host.with_release("v0.1.14");
            // The loop must return before its initial delay for every early-
            // exit config; a hang here fails the test via tokio's test timeout.
            let ctx = Ctx::new();
            tokio::select! {
                _ = auto_update_loop(&ctx, &host) => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    panic!("autoUpdateLoop did not exit early for case {name:?}");
                }
            }
        }
    }

    /// parse_self_version contract checks against cmd/cordy's template shape.
    #[test]
    fn parse_self_version_extracts_ldflags_field() {
        assert_eq!(
            parse_self_version("cordy 0.3.7 (commit: abc1234, built: 2026-07-29T10:00:00Z)\ngo: go1.26.1\n"),
            "0.3.7"
        );
        assert_eq!(parse_self_version("something else entirely"), "something else entirely");
        assert_eq!(parse_self_version(""), "");
    }

    /// is_release_version / is_newer_version strictness (cli/update.go).
    #[test]
    fn version_helpers_strictness() {
        assert!(is_release_version("0.1.13"));
        assert!(is_release_version("v0.1.13"));
        assert!(!is_release_version(""));
        assert!(!is_release_version("v0.2.15-235-gdaf0e935"));
        assert!(!is_release_version("0.1"));
        assert!(is_newer_version("v0.1.14", "v0.1.13"));
        assert!(!is_newer_version("v0.1.13", "v0.1.13"));
        assert!(!is_newer_version("v0.2.15-235-gdaf0e935", "v0.1.13"));
        assert!(!is_newer_version("garbage", "v0.1.13"));
    }
}
