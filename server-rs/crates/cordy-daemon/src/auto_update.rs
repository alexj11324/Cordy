//! Port of `server/internal/daemon/auto_update.go` (433 lines) plus the three
//! version helpers it imports from `server/internal/cli/update.go`.
//!
//! Symbol map (Go → Rust):
//! - `ParseSelfVersion` → [`parse_self_version`] (exported contract shared
//!   with cmd/cordy's version template)
//! - `fetchLatestRelease` / `isReleaseVersion` / `isNewerVersion` /
//!   `detectSelfVersion` → [`fetch_latest_release`] / [`is_release_version`] /
//!   [`is_newer_version`] / [`detect_self_version`] — hosted here until the
//!   CLI crate lands (S10); they belong to cli/update.go
//! - `autoUpdateLoop` / `tryAutoUpdate` / `trySelfReload` →
//!   [`auto_update_loop`] / [`try_auto_update`] / [`try_self_reload`]
//! - `setReloadPending` / `clearReloadPending` / `reloadPending` →
//!   host seam methods
//!
//! Port notes: the `*Daemon` receiver becomes the [`AutoUpdateHost`] trait
//! (same seam pattern as gc.rs's GcHost); config fields live in
//! [`AutoUpdateSettings`]. Go's package-level indirection vars become
//! [`AutoUpdateProbes`] so tests stub GitHub/process forks deterministically.
//! Deferred flag restores are written as explicit epilogues preserving the
//! exact hold/release order around triggerRestart.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use futures_util::future::BoxFuture;
use serde::Deserialize;

use crate::repocache::Ctx;

/// How long the loop waits before its first check (go:69): startup has auth,
/// register, sync, heartbeats already; also keeps brand-new installs updating
/// within a couple of minutes rather than a full interval.
const AUTO_UPDATE_INITIAL_DELAY: Duration = Duration::from_secs(2 * 60);

/// How often the running version is compared against the on-disk binary
/// (go:86). Machine-level fork/exec per tick; bounds how long a user waits
/// after replacing the binary themselves.
const SELF_RELOAD_CHECK_INTERVAL: Duration = Duration::from_secs(600);

/// Bounds the `--version` fork/exec (go:91): stops a wedged binary from
/// parking the loop goroutine, nothing more.
const SELF_RELOAD_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// DefaultAutoUpdateCheckInterval (config.go:79): how often the daemon polls
/// GitHub for a newer CLI release — the value that tracks release cadence.
pub(crate) const DEFAULT_AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Subset of cli.GitHubRelease the daemon consumes (update.go:33–37).
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    #[serde(rename = "tag_name", default)]
    pub tag_name: String,
    #[serde(rename = "html_url", default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubReleaseAsset {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "browser_download_url", default)]
    pub browser_download_url: String,
}

/// `IsReleaseVersion` (update.go:47–68): a tagged release looks like
/// "0.1.13"/"v0.1.13"; dev builds (`git describe` style) are rejected so a
/// source build never downgrades to a public release.
pub fn is_release_version(v: &str) -> bool {
    parse_release_version(v).is_some()
}

/// `IsNewerVersion` (update.go:73–87): strictly newer; both sides may carry a
/// "v" prefix; non-numeric tails make the side unparseable → false ("stay").
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let (Some(l), Some(c)) = (
        parse_release_version(latest),
        parse_release_version(current),
    ) else {
        return false;
    };
    l != c && l > c
}

/// `parseReleaseVersion` (update.go:94–124): exactly three all-numeric
/// components after an optional "v". The strict shape is intentional — this
/// is the only parser feeding IsNewerVersion.
fn parse_release_version(v: &str) -> Option<[u64; 3]> {
    let s = v.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut out = [0u64; 3];
    for (slot, part) in out.iter_mut().zip(&parts) {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // u64 parse can't fail after the all-digits check except on overflow;
        // overflow also means "not a sane version".
        *slot = part.parse().ok()?;
    }
    Some(out)
}

/// `ParseSelfVersion` (auto_update.go:54–61): pulls the version field out of
/// `cordy --version` output whose first line is rendered by cmd/cordy:
///
/// ```text
/// cordy 0.3.7 (commit: abc1234, built: 2026-07-29T10:00:00Z)
/// ```
///
/// The extracted value is exactly what ldflags put in main.version and what
/// lands in Config.CLIVersion, so the two compare directly. Anything not
/// matching the template shape is returned trimmed, which compares unequal
/// and is therefore reported rather than silently ignored.
pub fn parse_self_version(raw: &str) -> String {
    let line = raw.split('\n').next().unwrap_or("").trim();
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() >= 2 && fields[0] == "cordy" {
        return fields[1].to_string();
    }
    line.to_string()
}

/// `detectSelfVersion` (auto_update.go:25–38): runs `<path> --version`. The
/// processtree runner provides Go's WaitDelay guard — a replaced binary that
/// misbehaves cannot park the loop forever with the updating flag held.
async fn detect_self_version(ctx: &Ctx, path: &str) -> anyhow::Result<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg("--version");
    let output = crate::gc::processtree::output(ctx, cmd, Duration::from_secs(2))
        .await
        .map_err(|err| anyhow!("run {path} --version: {err}"))?;
    Ok(parse_self_version(&String::from_utf8_lossy(&output)))
}

/// `fetchLatestRelease` (update.go:255–274): latest cordy release from the
/// GitHub API.
async fn fetch_latest_release() -> anyhow::Result<Option<GitHubRelease>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| anyhow!("{err}"))?;
    let response = client
        .get("https://api.github.com/repos/cordy-ai/cordy/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| anyhow!("{err}"))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "GitHub API returned {}",
            response.status().as_u16()
        ));
    }
    let release: GitHubRelease = response.json().await.map_err(|err| anyhow!("{err}"))?;
    Ok(Some(release))
}

/// Config surface auto_update.go reads (mirrors internal/daemon/config.go
/// field names).
#[derive(Debug, Clone)]
pub(crate) struct AutoUpdateSettings {
    pub launched_by: String,
    pub cli_version: String,
    pub auto_update_enabled: bool,
    pub auto_reload_enabled: bool,
    pub auto_update_check_interval: Duration,
}

/// The `*Daemon` surface auto_update.go touches (trait seam, gc.rs GcHost
/// pattern). Integration wires this to the Daemon struct; tests supply fakes.
pub(crate) trait AutoUpdateHost: Send + Sync {
    fn settings(&self) -> &AutoUpdateSettings;

    /// `d.updating.Load()`.
    fn updating_load(&self) -> bool;
    /// `d.updating.CompareAndSwap(false, true)`.
    fn updating_cas_acquire(&self) -> bool;
    /// `d.updating.Store(false)`.
    fn updating_store_false(&self);

    /// `d.activeTasks.Load()` — ownership-safe count of tasks in handleTask.
    fn active_tasks(&self) -> i64;

    /// `d.trySetClaimBarrier` (daemon.go:4548): atomically pause new claims
    /// when the daemon is fully idle; false when busy or already held.
    fn try_set_claim_barrier(&self) -> bool;
    /// `d.releaseClaimBarrier` (daemon.go:4566).
    fn release_claim_barrier(&self);

    /// `d.RestartBinary()` (daemon.go:2038): scheduled restart target, empty
    /// when none.
    fn restart_binary(&self) -> String;

    /// `d.runUpdateFn`: executes the brew-or-download upgrade.
    fn run_update<'a>(&'a self, target_version: &'a str) -> BoxFuture<'a, anyhow::Result<String>>;

    /// `d.triggerRestart` (daemon.go:4580): schedule re-exec into the new
    /// binary and cancel the main context. False = target unresolvable.
    fn trigger_restart(&self) -> bool;

    /// `d.restartTargetBinary` (daemon.go:4617): the binary a restart would
    /// actually exec — under brew the stable symlink path, not the Cellar
    /// path os.Executable resolves to.
    fn restart_target_binary(&self) -> anyhow::Result<String>;

    /// `d.reloadPendingReason.Store`.
    fn set_reload_pending(&self, reason: Option<String>);
}

/// Go's package-level test indirections (`fetchLatestRelease`,
/// `detectSelfVersion`) as injectable closures with real defaults.
#[derive(Clone)]
pub(crate) struct AutoUpdateProbes {
    #[allow(clippy::type_complexity)]
    pub fetch_latest_release:
        Arc<dyn Fn() -> BoxFuture<'static, anyhow::Result<Option<GitHubRelease>>> + Send + Sync>,
}

impl AutoUpdateProbes {
    pub(crate) fn real() -> Self {
        Self {
            fetch_latest_release: Arc::new(|| Box::pin(fetch_latest_release())),
        }
    }
}

/// `autoUpdateLoop` (go:116–189): owns everything that can end in "restart
/// this daemon into a different cordy binary". Two independent checks on one
/// task so they can't race each other into triggerRestart. Both are skipped
/// for Desktop-managed daemons.
pub(crate) async fn auto_update_loop(
    host: &dyn AutoUpdateHost,
    ctx: &Ctx,
    probes: AutoUpdateProbes,
) {
    let settings = host.settings();
    if settings.launched_by == "desktop" {
        tracing::info!("auto-update: skipped (managed by Desktop)");
        return;
    }

    let mut pull_enabled = settings.auto_update_enabled;
    if !pull_enabled {
        tracing::info!("auto-update: disabled");
    } else if !is_release_version(&settings.cli_version) {
        tracing::info!(
            version = %settings.cli_version,
            "auto-update: skipped (not a release build)"
        );
        pull_enabled = false;
    }
    let reload_enabled = settings.auto_reload_enabled;
    if !reload_enabled {
        tracing::info!("auto-reload: disabled");
    }
    if !pull_enabled && !reload_enabled {
        return;
    }

    let mut pull_interval = settings.auto_update_check_interval;
    if pull_interval.is_zero() {
        pull_interval = DEFAULT_AUTO_UPDATE_CHECK_INTERVAL;
    }
    // Log what each half will do before the startup delay, so a user reading
    // daemon.log right after `daemon start` sees it immediately (go:142–150).
    if pull_enabled {
        tracing::info!(
            interval = go_duration(pull_interval),
            current = %settings.cli_version,
            "auto-update: started"
        );
    }
    if reload_enabled {
        tracing::info!(
            interval = go_duration(SELF_RELOAD_CHECK_INTERVAL),
            current = %settings.cli_version,
            "auto-reload: watching the cordy binary on disk"
        );
    }

    if crate::helpers::sleep_with_context(ctx, AUTO_UPDATE_INITIAL_DELAY)
        .await
        .is_err()
    {
        return;
    }
    if pull_enabled {
        try_auto_update(host, ctx, &probes).await;
    }
    if reload_enabled {
        try_self_reload(host, ctx).await;
    }

    // Tickers start only after the first check (go:162–177): starting them
    // earlier would buffer a tick and fire a second check immediately. A
    // disabled half never fires its branch.
    let mut pull_ticker = tokio::time::interval(pull_interval);
    pull_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    pull_ticker.tick().await; // consume the immediate first tick
    let mut reload_ticker = tokio::time::interval(SELF_RELOAD_CHECK_INTERVAL);
    reload_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    reload_ticker.tick().await;

    loop {
        tokio::select! {
            _ = ctx.cancelled() => return,
            _ = pull_ticker.tick(), if pull_enabled => try_auto_update(host, ctx, &probes).await,
            _ = reload_ticker.tick(), if reload_enabled => try_self_reload(host, ctx).await,
        }
    }
}

fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 || h > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&format!("{s}s"));
    out
}

/// `tryAutoUpdate` (go:197–291): one check-and-maybe-upgrade cycle; never
/// errors — a failed check retries at the next tick.
pub(crate) async fn try_auto_update(
    host: &dyn AutoUpdateHost,
    ctx: &Ctx,
    probes: &AutoUpdateProbes,
) {
    if ctx.err().is_some() {
        return;
    }
    // Don't race the server-triggered update path (go:201–208).
    if host.updating_load() {
        tracing::debug!("auto-update: skip — update already in progress");
        return;
    }
    // Cheap pre-fetch idle check: don't pay the GitHub call when we already
    // know we'll defer; a task starting after this load is caught by the
    // strict barrier check below (go:209–217).
    let running = host.active_tasks();
    if running > 0 {
        tracing::debug!(active = running, "auto-update: skip — tasks running");
        return;
    }

    let release = match (probes.fetch_latest_release)().await {
        Ok(release) => release,
        Err(err) => {
            tracing::warn!(error = %err, "auto-update: fetch latest release failed — will retry");
            return;
        }
    };
    let Some(release) = release.filter(|release| !release.tag_name.is_empty()) else {
        return;
    };
    if !is_newer_version(&release.tag_name, &host.settings().cli_version) {
        return;
    }

    // CAS the updating flag so a concurrent server-triggered handleUpdate
    // can't double-fire (go:231–244).
    if !host.updating_cas_acquire() {
        tracing::debug!("auto-update: skip — update already in progress (raced)");
        return;
    }

    // Strict barrier between the cheap check and the upgrade kick-off
    // (go:246–262). Both flags restore in Go via defers; the epilogue below
    // preserves the exact semantics: on a scheduled restart both stay held —
    // process exit is imminent and clearing either would open a window for
    // new claims mid-shutdown (go:284–290).
    let mut hold_through_exit = false;
    if !host.try_set_claim_barrier() {
        tracing::info!("auto-update: deferring — task or claim in flight at barrier check");
    } else {
        tracing::info!(
            current = %host.settings().cli_version,
            target = %release.tag_name,
            "auto-update: newer release available, upgrading"
        );
        match host.run_update(&release.tag_name).await {
            Ok(output) => {
                tracing::info!(
                    target = %release.tag_name,
                    output = %output,
                    "auto-update: upgrade completed, restarting"
                );
                if host.trigger_restart() {
                    hold_through_exit = true;
                } else {
                    // The upgrade landed but no exit is coming; holding either
                    // guard would stop this daemon claiming forever (go:275–282).
                    tracing::error!(
                        "auto-update: upgrade completed but restart could not be scheduled — resuming claims"
                    );
                    host.release_claim_barrier();
                }
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "auto-update: upgrade failed — will retry"
                );
                host.release_claim_barrier();
            }
        }
    }
    if !hold_through_exit {
        host.updating_store_false();
    }
}

/// `trySelfReload` (go:326–411): restarts the daemon when the cordy binary on
/// disk no longer reports the version compiled into this process. Covers
/// out-of-band replacement (`brew upgrade`, re-download, `make build`) that
/// auto-update can't recover. A failed probe is never treated as a change.
pub(crate) async fn try_self_reload(host: &dyn AutoUpdateHost, ctx: &Ctx) {
    if ctx.err().is_some() {
        return;
    }
    if !host.restart_binary().is_empty() {
        return;
    }
    // Acquire ownership rather than sampling it — only the CAS arbitrates
    // against handleUpdate (go:333–354).
    if !host.updating_cas_acquire() {
        tracing::debug!("auto-reload: skip — update already in progress");
        return;
    }
    'attempt: {
        let release_barrier_early = |host: &dyn AutoUpdateHost| {
            host.updating_store_false();
        };
        let target = match host.restart_target_binary() {
            Ok(target) => target,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "auto-reload: could not resolve own executable — will retry"
                );
                release_barrier_early(host);
                break 'attempt;
            }
        };
        let probe_ctx = ctx.child();
        let probe = tokio::time::timeout(
            SELF_RELOAD_PROBE_TIMEOUT,
            detect_self_version(&probe_ctx, &target),
        );
        let on_disk = match probe.await {
            Ok(Ok(on_disk)) => on_disk,
            Ok(Err(err)) => {
                tracing::warn!(
                    binary = %target,
                    error = %err,
                    "auto-reload: version probe failed — will retry, not treating it as a change"
                );
                release_barrier_early(host);
                break 'attempt;
            }
            Err(_) => {
                tracing::warn!(
                    binary = %target,
                    "auto-reload: version probe timed out — will retry, not treating it as a change"
                );
                release_barrier_early(host);
                break 'attempt;
            }
        };
        // Either side blank makes the comparison meaningless (go:372–381).
        if on_disk.is_empty() || host.settings().cli_version.is_empty() {
            tracing::warn!(
                binary = %target,
                on_disk = %on_disk,
                running = %host.settings().cli_version,
                "auto-reload: version unavailable — will retry, not treating it as a change"
            );
            release_barrier_early(host);
            break 'attempt;
        }
        if on_disk == host.settings().cli_version {
            host.set_reload_pending(None);
            release_barrier_early(host);
            break 'attempt;
        }

        let reason = format!(
            "cordy binary on disk reports {on_disk}, running {}",
            host.settings().cli_version
        );
        host.set_reload_pending(Some(reason.clone()));

        if !host.try_set_claim_barrier() {
            tracing::info!(
                reason = %reason,
                "auto-reload: deferring — task or claim in flight at barrier check"
            );
            release_barrier_early(host);
            break 'attempt;
        }
        tracing::info!(
            reason = %reason,
            binary = %target,
            "auto-reload: restarting into the binary on disk"
        );
        if host.trigger_restart() {
            // Process exit is imminent; leave both the flag and barrier held.
        } else {
            // A failed restart must never cost the daemon its claims nor hold
            // the flag against a restart that is not coming (go:402–408).
            host.release_claim_barrier();
            host.updating_store_false();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
    use std::sync::Mutex;

    type RunUpdateFn = Box<dyn Fn(&str) -> (String, anyhow::Result<String>) + Send + Sync>;

    /// Daemon stripped to the pieces tryAutoUpdate/trySelfReload touch
    /// (auto_update_test.go:18–33).
    struct FakeHost {
        settings: AutoUpdateSettings,
        updating: AtomicBool,
        active_tasks: AtomicI64,
        claims_in_flight: AtomicI64,
        pause_claims: AtomicBool,
        restart_binary: Mutex<String>,
        reload_pending: Mutex<Option<String>>,
        restart_calls: AtomicUsize,
        run_update: Mutex<Option<RunUpdateFn>>,
    }

    impl FakeHost {
        fn new(current_version: &str) -> Self {
            Self {
                settings: AutoUpdateSettings {
                    launched_by: String::new(),
                    cli_version: current_version.to_string(),
                    auto_update_enabled: true,
                    auto_reload_enabled: false,
                    auto_update_check_interval: Duration::ZERO,
                },
                updating: AtomicBool::new(false),
                active_tasks: AtomicI64::new(0),
                claims_in_flight: AtomicI64::new(0),
                pause_claims: AtomicBool::new(false),
                restart_binary: Mutex::new(String::new()),
                reload_pending: Mutex::new(None),
                restart_calls: AtomicUsize::new(0),
                run_update: Mutex::new(None),
            }
        }

        fn pause_claims(&self) -> bool {
            self.pause_claims.load(Ordering::SeqCst)
        }
    }

    impl AutoUpdateHost for FakeHost {
        fn settings(&self) -> &AutoUpdateSettings {
            &self.settings
        }
        fn updating_load(&self) -> bool {
            self.updating.load(Ordering::SeqCst)
        }
        fn updating_cas_acquire(&self) -> bool {
            self.updating
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
        fn updating_store_false(&self) {
            self.updating.store(false, Ordering::SeqCst);
        }
        fn active_tasks(&self) -> i64 {
            self.active_tasks.load(Ordering::SeqCst)
        }
        // Mirrors daemon.go trySetClaimBarrier: refuse when already held or
        // busy; pairs with tryEnterClaim's counter in production.
        fn try_set_claim_barrier(&self) -> bool {
            if self.pause_claims()
                || self.claims_in_flight.load(Ordering::SeqCst) > 0
                || self.active_tasks() > 0
            {
                return false;
            }
            self.pause_claims.store(true, Ordering::SeqCst);
            true
        }
        fn release_claim_barrier(&self) {
            self.pause_claims.store(false, Ordering::SeqCst);
        }
        fn restart_binary(&self) -> String {
            self.restart_binary.lock().unwrap().clone()
        }
        fn run_update<'a>(
            &'a self,
            target_version: &'a str,
        ) -> BoxFuture<'a, anyhow::Result<String>> {
            Box::pin(async move {
                let guard = self.run_update.lock().unwrap();
                match guard.as_ref() {
                    Some(runner) => {
                        let (output, result) = runner(target_version);
                        let _ = output;
                        result
                    }
                    None => panic!("runUpdateFn called unexpectedly"),
                }
            })
        }
        fn trigger_restart(&self) -> bool {
            self.restart_calls.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn restart_target_binary(&self) -> anyhow::Result<String> {
            Ok("/usr/local/bin/cordy".to_string())
        }
        fn set_reload_pending(&self, reason: Option<String>) {
            *self.reload_pending.lock().unwrap() = reason;
        }
    }

    fn stub_release(tag: Option<&str>, err: Option<&str>) -> AutoUpdateProbes {
        let tag = tag.map(str::to_string);
        let err = err.map(str::to_string);
        AutoUpdateProbes {
            fetch_latest_release: Arc::new(move || {
                let tag = tag.clone();
                let err = err.clone();
                Box::pin(async move {
                    if let Some(message) = err {
                        return Err(anyhow!(message));
                    }
                    Ok(tag.map(|tag_name| GitHubRelease {
                        tag_name,
                        html_url: String::new(),
                        assets: Vec::new(),
                    }))
                })
            }),
        }
    }

    async fn try_auto_update_with(host: &FakeHost, probes: &AutoUpdateProbes) {
        try_auto_update(host, &Ctx::new(), probes).await;
    }

    #[tokio::test]
    async fn skips_when_another_update_is_in_progress() {
        let host = FakeHost::new("v0.1.13");
        host.updating.store(true, Ordering::SeqCst);
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn skips_when_tasks_running() {
        let host = FakeHost::new("v0.1.13");
        host.active_tasks.store(1, Ordering::SeqCst);
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
        assert!(!host.updating.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn defers_when_claim_in_flight_at_barrier() {
        let host = FakeHost::new("v0.1.13");
        host.claims_in_flight.store(1, Ordering::SeqCst);
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
        assert!(
            !host.updating.load(Ordering::SeqCst),
            "updating flag must be released after a deferred upgrade"
        );
        assert!(
            !host.pause_claims(),
            "pauseClaims must be cleared after a deferred upgrade"
        );
    }

    #[tokio::test]
    async fn holds_barrier_across_restart() {
        let host = FakeHost::new("v0.1.13");
        *host.run_update.lock().unwrap() = Some(Box::new(|_| {
            ("upgraded".to_string(), Ok("upgraded".to_string()))
        }));
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 1);
        assert!(
            host.pause_claims(),
            "pauseClaims must remain set across the restart kick"
        );
    }

    #[tokio::test]
    async fn releases_barrier_on_upgrade_failure() {
        let host = FakeHost::new("v0.1.13");
        *host.run_update.lock().unwrap() = Some(Box::new(|_| {
            (
                "brew network error".to_string(),
                Err(anyhow!("brew upgrade failed")),
            )
        }));
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
        assert!(!host.pause_claims());
    }

    #[test]
    fn try_enter_claim_respects_barrier_shape() {
        // The barrier contract the poller side relies on (go:136–158): enter/
        // exit balance and refusal while held. Exercised through the fake's
        // mirror of trySetClaimBarrier.
        let host = FakeHost::new("");
        assert!(host.try_set_claim_barrier());
        assert!(!{
            host.claims_in_flight.store(1, Ordering::SeqCst);
            let granted = host.try_set_claim_barrier();
            host.claims_in_flight.store(0, Ordering::SeqCst);
            granted
        });
        host.release_claim_barrier();
        assert!(host.try_set_claim_barrier());
        host.release_claim_barrier();
    }

    #[tokio::test]
    async fn skips_when_fetch_fails() {
        let host = FakeHost::new("v0.1.13");
        try_auto_update_with(&host, &stub_release(None, Some("network down"))).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn skips_when_not_newer() {
        let host = FakeHost::new("v0.1.13");
        try_auto_update_with(&host, &stub_release(Some("v0.1.13"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn runs_upgrade_and_restarts_on_newer() {
        let upgraded_to = Arc::new(Mutex::new(String::new()));
        let host = FakeHost::new("v0.1.13");
        let captured = Arc::clone(&upgraded_to);
        *host.run_update.lock().unwrap() = Some(Box::new(move |target| {
            *captured.lock().unwrap() = target.to_string();
            ("upgraded".to_string(), Ok("upgraded".to_string()))
        }));
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(*upgraded_to.lock().unwrap(), "v0.1.14");
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 1);
        assert!(
            host.updating.load(Ordering::SeqCst),
            "updating flag should remain set across the restart kick"
        );
    }

    #[tokio::test]
    async fn does_not_restart_on_upgrade_failure() {
        let host = FakeHost::new("v0.1.13");
        *host.run_update.lock().unwrap() = Some(Box::new(|_| {
            (
                "brew: network error".to_string(),
                Err(anyhow!("brew upgrade failed")),
            )
        }));
        try_auto_update_with(&host, &stub_release(Some("v0.1.14"), None)).await;
        assert_eq!(host.restart_calls.load(Ordering::SeqCst), 0);
        assert!(
            !host.updating.load(Ordering::SeqCst),
            "updating flag must be released after a failed upgrade"
        );
    }

    #[tokio::test]
    async fn loop_early_exits() {
        struct NullHost(AutoUpdateSettings);
        impl AutoUpdateHost for NullHost {
            fn settings(&self) -> &AutoUpdateSettings {
                &self.0
            }
            fn updating_load(&self) -> bool {
                false
            }
            fn updating_cas_acquire(&self) -> bool {
                false
            }
            fn updating_store_false(&self) {}
            fn active_tasks(&self) -> i64 {
                0
            }
            fn try_set_claim_barrier(&self) -> bool {
                false
            }
            fn release_claim_barrier(&self) {}
            fn restart_binary(&self) -> String {
                String::new()
            }
            fn run_update<'a>(&'a self, _: &'a str) -> BoxFuture<'a, anyhow::Result<String>> {
                panic!("runUpdateFn called from an early-exit code path")
            }
            fn trigger_restart(&self) -> bool {
                false
            }
            fn restart_target_binary(&self) -> anyhow::Result<String> {
                Ok(String::new())
            }
            fn set_reload_pending(&self, _: Option<String>) {}
        }
        let cases = vec![
            ("disabled by config", false, "v0.1.13", ""),
            ("managed by desktop", true, "v0.1.13", "desktop"),
            ("dev build", true, "v0.1.13-235-gabcdef0", ""),
        ];
        for (_, enabled, version, launched_by) in cases {
            let host = NullHost(AutoUpdateSettings {
                launched_by: launched_by.to_string(),
                cli_version: version.to_string(),
                auto_update_enabled: enabled,
                auto_reload_enabled: false,
                auto_update_check_interval: Duration::ZERO,
            });
            auto_update_loop(&host, &Ctx::new(), stub_release(Some("v0.1.14"), None)).await;
        }
    }

    #[test]
    fn parse_self_version_extracts_ldflags_field() {
        assert_eq!(
            parse_self_version("cordy 0.3.7 (commit: abc1234, built: 2026-07-29T10:00:00Z)\ngo: go1.26.1, os/arch: darwin/arm64\n"),
            "0.3.7"
        );
        assert_eq!(parse_self_version("weird output\n"), "weird output");
        assert_eq!(parse_self_version(""), "");
    }

    #[test]
    fn release_version_classification() {
        assert!(is_release_version("0.1.13"));
        assert!(is_release_version("v0.1.13"));
        assert!(!is_release_version(""));
        assert!(!is_release_version("v0.2.15-235-gdaf0e935"));
        assert!(!is_release_version("1.2"));
        assert!(!is_release_version("1.2.x"));
    }

    #[test]
    fn newer_version_comparison() {
        assert!(is_newer_version("v0.1.14", "v0.1.13"));
        assert!(is_newer_version("0.2.0", "v0.1.99"));
        assert!(!is_newer_version("v0.1.13", "v0.1.13"));
        assert!(!is_newer_version("v0.1.12", "v0.1.13"));
        // Unparseable either side → stay on current.
        assert!(!is_newer_version("", "v0.1.13"));
        assert!(!is_newer_version("v0.2.15-235-gdaf0e935", "v0.1.13"));
    }
}
