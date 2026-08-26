#![allow(dead_code)]
// S9-integration: strategy half of agents_refresh.go; loop bodies wire in with daemon.go core (lane-B final)
//! Port of `server/internal/daemon/agents_refresh.go` — the discovery loop
//! policy that keeps the registered runtime set converged on the agent CLIs
//! actually installed on this machine (MUL-5439).
//!
//! The file splits in two. This module carries the **strategy half** — the
//! parts whose behavior does not touch the Daemon struct: the tick intervals,
//! the converge retry/backoff policy, the one-directional availability merge,
//! the missing-provider and version-lag scans, the demotion partition, and the
//! confirmed runtime verdict type. They are free functions over injected
//! state, in the [`GcHost`] / [`AutoUpdateHost`] trait-seam pattern, so the
//! daemon wiring calls them directly instead of re-deriving them.
//!
//! Rust's registration service now owns the migrated version-refresh and
//! demotion orchestration: it applies the lag scan through the authoritative
//! registry, holds the claim barrier, performs structured deregistration, and
//! runs beside workspace reconciliation from the production refresh loop.
//! Remaining daemon-wide capabilities continue to migrate by complete
//! business boundary; this module no longer claims that version/demotion
//! orchestration is still implemented in Go.
//!
//! Symbol map (Go → Rust):
//! - `agentDiscoveryInterval` → [`AGENT_DISCOVERY_INTERVAL`]
//! - `agentConvergeMaxBackoff` → [`AGENT_CONVERGE_MAX_BACKOFF`]
//! - `agentVersionRefreshInterval` → [`AGENT_VERSION_REFRESH_INTERVAL`]
//! - `nextConvergeBackoff` → [`next_converge_backoff`]
//! - loop tick decision + backoff bookkeeping → [`ConvergeRetryState`]
//! - `d.agents` / `setSkippedAgents` / `skippedAgentsSnapshot` →
//!   [`AgentsRefreshHost`]
//! - `refreshAgentAvailability` merge half → [`merge_discovered_agents`] /
//!   [`gained_providers`] / [`refresh_agent_availability`]
//! - `providersMissingRuntimes` scan → [`providers_missing_runtimes`]
//! - `runtimeVerdict` → [`RuntimeVerdict`] (constructed by the provider probe)
//! - `revivedRuntimes`(+`reasonsFor`) → [`RevivedRuntimes`]
//! - `builtinVersionsFromPayload` → [`builtin_versions_from_payload`]
//! - `refreshAgentVersions` lag scan → [`workspaces_behind_on_versions`]
//! - `demoteUnusableRuntimes` d.mu section → [`partition_demotable_runtimes`]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use crate::types::{AgentEntry, Runtime};

// ---------------------------------------------------------------------------
// Intervals (agents_refresh.go:10–43)
// ---------------------------------------------------------------------------

/// `agentDiscoveryInterval` (go:15): how often a running daemon re-checks
/// which agent CLIs are installed. A round is a handful of exec.LookPath
/// calls — the login-shell fallback is separately rate-limited by the much
/// longer shellResolveTTL — so this can be short enough that installing a CLI
/// feels immediate.
pub(crate) const AGENT_DISCOVERY_INTERVAL: Duration = Duration::from_secs(2 * 60);

/// `agentConvergeMaxBackoff` (go:23): caps the retry delay for a discovered
/// provider that keeps failing to register. Discovery itself stays on
/// [`AGENT_DISCOVERY_INTERVAL`]; only the expensive half — version probes plus
/// one register call per workspace — backs off.
pub(crate) const AGENT_CONVERGE_MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// `agentVersionRefreshInterval` (go:43): how often a running daemon re-probes
/// the version of every agent CLI it already has registered, so an in-place
/// upgrade is picked up without a restart. Deliberately tracks
/// `selfReloadCheckInterval` rather than being pushed out on cost grounds: it
/// is also the window in which an unsupported CLI keeps claiming tasks.
pub(crate) const AGENT_VERSION_REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// `nextConvergeBackoff` (go:105): doubles the retry delay, starting at one
/// discovery interval and capped at [`AGENT_CONVERGE_MAX_BACKOFF`].
pub(crate) fn next_converge_backoff(current: Duration) -> Duration {
    if current.is_zero() {
        return AGENT_DISCOVERY_INTERVAL;
    }
    let next = current * 2;
    if next > AGENT_CONVERGE_MAX_BACKOFF {
        AGENT_CONVERGE_MAX_BACKOFF
    } else {
        next
    }
}

// ---------------------------------------------------------------------------
// Loop tick policy (agents_refresh.go:69–99)
// ---------------------------------------------------------------------------

/// The discovery loop's retry bookkeeping (go:69–72, 82–98): whether a tick
/// may attempt a convergence, and how the backoff evolves afterwards.
///
/// Extracted because the surrounding loop cannot port until daemon.go core
/// lands, while the *policy* is fully specified today — a newly discovered
/// provider always gets an immediate attempt; otherwise the backoff earned by
/// previous failures applies; progress resets it; a missing set going empty
/// clears it.
#[derive(Debug, Clone)]
pub(crate) struct ConvergeRetryState {
    backoff: Duration,
    next_retry: Option<Instant>,
}

impl ConvergeRetryState {
    pub(crate) fn new() -> Self {
        Self {
            backoff: Duration::ZERO,
            next_retry: None,
        }
    }

    /// Current backoff — exposed for diagnostics parity with Go's captured
    /// local of the same name.
    pub(crate) fn backoff(&self) -> Duration {
        self.backoff
    }

    /// Go: `if len(gained) == 0 && now.Before(nextRetry) { continue }`.
    /// A newly discovered provider always gets an immediate attempt.
    pub(crate) fn should_attempt(&self, gained_any: bool, now: Instant) -> bool {
        gained_any || self.next_retry.is_none_or(|t| now >= t)
    }

    /// Go:93–98 — after a convergence attempt: progress resets the backoff,
    /// otherwise it doubles via [`next_converge_backoff`]; either way
    /// `nextRetry = now + backoff`.
    pub(crate) fn record_attempt(&mut self, progressed: bool, now: Instant) {
        self.backoff = if progressed {
            Duration::ZERO
        } else {
            next_converge_backoff(self.backoff)
        };
        self.next_retry = Some(now + self.backoff);
    }

    /// Go:82–85 — the missing set went empty: clear the earned backoff. Go
    /// leaves `nextRetry` untouched here; the next outcome recomputes it.
    pub(crate) fn reset_backoff(&mut self) {
        self.backoff = Duration::ZERO;
    }
}

impl Default for ConvergeRetryState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Availability set surface (agents_refresh.go:116–156)
// ---------------------------------------------------------------------------

/// The availability-set slice of the Daemon that the portable half touches.
/// Integration wires this onto the real Daemon; tests supply fakes.
pub(crate) trait AgentsRefreshHost: Send + Sync {
    /// `d.agentsAvailable.Load()` — `None` before the first publish.
    fn stored_agents(&self) -> Option<BTreeMap<String, AgentEntry>>;
    /// `d.cfg.Agents` — the startup config fallback.
    fn startup_agents(&self) -> BTreeMap<String, AgentEntry>;
    /// `d.agentsAvailable.Store(&merged)` — copy-on-write publish.
    fn publish_agents(&self, merged: BTreeMap<String, AgentEntry>);
    /// `setSkippedAgents` (go:138): replace the diagnostic
    /// "discovered but not registered" set reported on /health.
    fn set_skipped_agents(&self, skipped: BTreeMap<String, String>);
    /// `skippedAgentsSnapshot` (go:145): copy for the health handler.
    fn skipped_agents_snapshot(&self) -> BTreeMap<String, String>;

    /// `d.agents()` (go:121): the current built-in availability set, falling
    /// back to the startup config when nothing was ever published (zero-value
    /// Daemon in tests).
    fn agents(&self) -> BTreeMap<String, AgentEntry> {
        match self.stored_agents() {
            Some(m) => m,
            None => self.startup_agents(),
        }
    }
}

// ---------------------------------------------------------------------------
// refreshAgentAvailability (agents_refresh.go:158–198)
// ---------------------------------------------------------------------------

/// Providers present in `probed` but not in `current`, sorted (go:174–183).
/// Sorted because the result feeds logs and tests.
pub(crate) fn gained_providers(
    current: &BTreeMap<String, AgentEntry>,
    probed: &BTreeMap<String, AgentEntry>,
) -> Vec<String> {
    let mut gained: Vec<String> = probed
        .keys()
        .filter(|name| !current.contains_key(*name))
        .cloned()
        .collect();
    gained.sort();
    gained
}

/// The copy-on-write merge half of `refreshAgentAvailability` (go:185–194):
/// build the union of the known set and the freshly probed one. Entries we
/// already knew about are preserved as-is so this never fights the pinned
/// path / self-heal bookkeeping for a running provider. Returns `None` when
/// nothing was gained — the caller must not republish an unchanged map
/// (unlocked readers rely on the published value being stable across no-op
/// rounds).
pub(crate) fn merge_discovered_agents(
    current: &BTreeMap<String, AgentEntry>,
    probed: &BTreeMap<String, AgentEntry>,
) -> Option<BTreeMap<String, AgentEntry>> {
    if gained_providers(current, probed).is_empty() {
        return None;
    }
    let mut merged = current.clone();
    for name in probed.keys() {
        // Only genuinely new providers overwrite anything; existing entries
        // stay byte-identical (go:189–191).
        merged
            .entry(name.clone())
            .or_insert_with(|| probed[name].clone());
    }
    Some(merged)
}

/// `refreshAgentAvailability` (go:170–198): re-run CLI discovery and publish
/// providers that appeared since the last probe. Performs no registration.
///
/// Deliberately one-directional: only providers GAINED are acted on. A
/// provider that stops resolving is kept — removal stays the job of an
/// explicit restart, where the user chose the environment.
///
/// Returns the gained providers (sorted), for logging and tests.
pub(crate) fn refresh_agent_availability(host: &dyn AgentsRefreshHost) -> Vec<String> {
    let current = host.agents();
    let probed = crate::agents_probe::probe_agent_clis();

    let Some(merged) = merge_discovered_agents(&current, &probed) else {
        return Vec::new();
    };
    let gained = gained_providers(&current, &probed);
    host.publish_agents(merged);
    tracing::info!(providers = ?gained, "agent CLI discovered after startup");
    gained
}

// ---------------------------------------------------------------------------
// providersMissingRuntimes (agents_refresh.go:529–577)
// ---------------------------------------------------------------------------

/// The registration state one tracked workspace contributes to the
/// missing-provider scan: `ws.runtimeIDs` plus the shared `d.runtimeIndex`
/// they resolve through. Kept as raw ids + index so the profile-row filter
/// below stays inside the ported logic, exactly where Go has it.
pub(crate) type WorkspaceRuntimeIds = (String, Vec<String>);

/// `providersMissingRuntimes` scan (go:541–577): the discovered providers
/// that do not have a built-in runtime registered for every tracked
/// workspace. Custom profile runtimes (ProfileID set) are ignored, and rows
/// missing from the index count as not-registered. Returns nothing when no
/// workspace is tracked or nothing is available. Output is sorted.
pub(crate) fn providers_missing_runtimes(
    available: &BTreeMap<String, AgentEntry>,
    workspaces: &[WorkspaceRuntimeIds],
    runtime_index: &BTreeMap<String, Runtime>,
) -> Vec<String> {
    if available.is_empty() || workspaces.is_empty() {
        return Vec::new();
    }
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for (_, runtime_ids) in workspaces {
        let mut registered: BTreeSet<String> = BTreeSet::new();
        for rid in runtime_ids {
            let Some(rt) = runtime_index.get(rid) else {
                continue;
            };
            if !rt.profile_id.is_empty() {
                continue;
            }
            registered.insert(rt.provider.clone());
        }
        for name in available.keys() {
            if !registered.contains(name) {
                missing.insert(name.clone());
            }
        }
    }
    missing.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Version payload + lag scan (daemon.go:706 / agents_refresh.go:275–313)
// ---------------------------------------------------------------------------

/// `builtinVersionsFromPayload` (daemon.go:706–716): extracts provider ->
/// version from a registration payload's BUILT-IN entries (map keys
/// `type` / `version`; `profile_id` set means a custom profile entry, which is
/// not version-tracked — the drift path owns its lifecycle). Takes the same
/// string-map shape Go does because the payload is dynamic there too.
pub(crate) fn builtin_versions_from_payload(
    runtimes: &[HashMap<String, String>],
) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(runtimes.len());
    for rt in runtimes {
        if rt.get("profile_id").is_some_and(|v| !v.is_empty()) {
            continue;
        }
        out.insert(
            rt.get("type").cloned().unwrap_or_default(),
            rt.get("version").cloned().unwrap_or_default(),
        );
    }
    out
}

/// The lag scan inside `refreshAgentVersions` (go:277–311): a workspace is
/// behind when some carried provider's last accepted register call for it
/// carried a different version. A provider with NO record for a workspace is
/// skipped — it has never registered there, so bringing it online is the
/// converge path's job, not a version change. A failed probe is absent from
/// `carried`, and only carried providers are compared — that is what stops an
/// uninstalled CLI from triggering registrations forever.
///
/// `workspaces` maps workspace id -> `ws.builtinVersions` (the versions the
/// last accepted register call actually carried).
///
/// Returns `(behind, transitions)` — workspace ids sorted, and human-readable
/// transition descriptions (`"provider sent -> version"`, or `"provider
/// version"` on first sighting) sorted, matching Go's log fields.
pub(crate) fn workspaces_behind_on_versions(
    carried: &HashMap<String, String>,
    workspaces: &BTreeMap<String, HashMap<String, String>>,
) -> (Vec<String>, Vec<String>) {
    let mut transitions: BTreeSet<String> = BTreeSet::new();
    let mut behind: Vec<String> = Vec::new();
    for (id, sent_versions) in workspaces {
        let mut lagging = false;
        // Iterate `carried` in key order so transition insertion is
        // deterministic before the sort below (Go iterates the map randomly
        // and sorts the collected slice instead — same output).
        for (provider, version) in carried.iter().collect::<BTreeMap<_, _>>() {
            let Some(sent) = sent_versions.get(provider) else {
                continue;
            };
            if sent == version {
                continue;
            }
            lagging = true;
            if sent.is_empty() {
                transitions.insert(format!("{provider} {version}"));
            } else {
                transitions.insert(format!("{provider} {sent} -> {version}"));
            }
        }
        if lagging {
            behind.push(id.clone());
        }
    }
    behind.sort();
    (behind, transitions.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Demotion partition (agents_refresh.go:344–468, d.mu section go:395–442)
// ---------------------------------------------------------------------------

/// `runtimeVerdict` (daemon.go:2230–2233): the confirmed verdict a re-probe
/// reached about a provider's binary on disk. Construction (`newRuntimeVerdict`,
/// daemon.go:2237, incl. the ExecFormatRepair lookup) ports with
/// `detectBuiltinRuntimes` in daemon.go core; consumers only need these two
/// fields.
#[derive(Debug, Clone, Default)]
pub struct RuntimeVerdict {
    /// Evidence against the provider — user-visible in skipped_agents.
    pub reason: String,
    /// Stable code + repair command clients act on; `None` for verdicts that
    /// do not need client-visible explanation.
    pub offline: Option<crate::client::RuntimeOfflineReason>,
}

/// Everything `demoteUnusableRuntimes` computes under `d.mu` (go:396–442),
/// grouped for the callers that follow: the lock-free apply writes the kept
/// sets back, then each workspace's cleanup runs under its own register lock.
#[derive(Debug, Clone, Default)]
pub(crate) struct DemotionPartition {
    /// Every dropped runtime id across workspaces (go `demoted`).
    pub demoted_ids: Vec<String>,
    /// Per-workspace ids to deregister (go `demotedByWorkspace`).
    pub demoted_by_workspace: BTreeMap<String, Vec<String>>,
    /// Provider → evidence reason (go `demotedProviders`).
    pub demoted_providers: BTreeMap<String, String>,
    /// Runtime id → offline reason carried to the server per row (go
    /// `offlineReasons`) — only for verdicts that carry one (MUL-6164).
    pub offline_reasons: BTreeMap<String, crate::client::RuntimeOfflineReason>,
    /// (workspace, provider) `ws.builtinVersions` records to delete: the
    /// runtime is gone, so the record of what was registered for it goes too;
    /// converge re-seeds it on recovery (go:429–433).
    pub dropped_version_records: Vec<(String, String)>,
}

/// The `d.mu` critical section of `demoteUnusableRuntimes`, pure (go:395–442):
/// walk every tracked workspace's runtime ids and split them into the rows to
/// keep versus the rows a confirmed verdict condemns.
///
/// Rules carried over verbatim:
/// - profile runtimes (`ProfileID` set) are never touched — they have their
///   own lifecycle;
/// - rows missing from `runtime_index` are kept (same as Go's `!ok` branch);
/// - only providers named in `causes` are demoted, and only on a CONFIRMED
///   verdict — a probe that merely failed never appears there;
/// - fresh output vectors throughout: callers may hand out the kept lists
///   while other readers still hold the old ones (go:402–406).
///
/// Returns `(kept per workspace, partition)`; `markProvidersDemotedLocked`
/// remains a Daemon-side step because it guards future apply paths.
pub(crate) fn partition_demotable_runtimes(
    workspaces: &BTreeMap<String, Vec<String>>,
    runtime_index: &BTreeMap<String, Runtime>,
    causes: &BTreeMap<String, RuntimeVerdict>,
) -> (BTreeMap<String, Vec<String>>, DemotionPartition) {
    let mut kept_by_workspace: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut part = DemotionPartition::default();
    for (workspace_id, runtime_ids) in workspaces {
        let mut kept = Vec::with_capacity(runtime_ids.len());
        for rid in runtime_ids {
            let Some(rt) = runtime_index.get(rid) else {
                kept.push(rid.clone());
                continue;
            };
            if !rt.profile_id.is_empty() {
                kept.push(rid.clone());
                continue;
            }
            let Some(cause) = causes.get(&rt.provider) else {
                kept.push(rid.clone());
                continue;
            };
            part.demoted_ids.push(rid.clone());
            part.demoted_by_workspace
                .entry(workspace_id.clone())
                .or_default()
                .push(rid.clone());
            part.demoted_providers
                .insert(rt.provider.clone(), cause.reason.clone());
            if let Some(offline) = &cause.offline {
                part.offline_reasons.insert(rid.clone(), offline.clone());
            }
            part.dropped_version_records
                .push((workspace_id.clone(), rt.provider.clone()));
        }
        kept_by_workspace.insert(workspace_id.clone(), kept);
    }
    (kept_by_workspace, part)
}

// ---------------------------------------------------------------------------
// revivedRuntimes (daemon.go:1601–1628, consumed by agents_refresh.go:480)
// ---------------------------------------------------------------------------

/// `revivedRuntimes`: the rows a register response brought back for a provider
/// the daemon has already condemned — the ids to take offline again, and the
/// cause to re-attach per row because that register's upsert just overwrote it.
#[derive(Debug, Clone, Default)]
pub(crate) struct RevivedRuntimes {
    pub ids: Vec<String>,
    pub reasons: HashMap<String, crate::client::RuntimeOfflineReason>,
}

impl RevivedRuntimes {
    /// `reasonsFor` (daemon.go:1613): narrow the causes to the rows actually
    /// being deregistered. Sending a cause for a row we are no longer taking
    /// offline would attach it to a healthy runtime.
    pub(crate) fn reasons_for(
        &self,
        runtime_ids: &[String],
    ) -> HashMap<String, crate::client::RuntimeOfflineReason> {
        if self.reasons.is_empty() {
            return HashMap::new();
        }
        let mut out = HashMap::with_capacity(runtime_ids.len());
        for id in runtime_ids {
            if let Some(reason) = self.reasons.get(id) {
                out.insert(id.clone(), reason.clone());
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Tests — the strategy contracts the loop tests (agents_refresh_test.go)
// exercise through fixtures, asserted here directly.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn entry(path: &str) -> AgentEntry {
        AgentEntry {
            path: path.to_string(),
            ..Default::default()
        }
    }

    fn runtime(id: &str, provider: &str, profile_id: &str) -> (String, Runtime) {
        (
            id.to_string(),
            Runtime {
                id: id.to_string(),
                name: id.to_string(),
                provider: provider.to_string(),
                status: "online".to_string(),
                profile_id: profile_id.to_string(),
            },
        )
    }

    #[test]
    fn next_converge_backoff_starts_at_interval_and_caps() {
        assert_eq!(
            next_converge_backoff(Duration::ZERO),
            AGENT_DISCOVERY_INTERVAL
        );
        assert_eq!(
            next_converge_backoff(AGENT_DISCOVERY_INTERVAL),
            AGENT_DISCOVERY_INTERVAL * 2
        );
        // Doubling walks 2m → 4m → 8m → 16m → 30m(capped) and stays there.
        let mut b = Duration::ZERO;
        for _ in 0..5 {
            b = next_converge_backoff(b);
        }
        assert_eq!(b, AGENT_CONVERGE_MAX_BACKOFF);
        assert_eq!(
            next_converge_backoff(AGENT_CONVERGE_MAX_BACKOFF),
            AGENT_CONVERGE_MAX_BACKOFF
        );
    }

    #[test]
    fn retry_state_immediate_attempt_on_gain_then_defers() {
        let mut st = ConvergeRetryState::new();
        let t0 = Instant::now();
        assert!(st.should_attempt(true, t0));
        st.record_attempt(false, t0); // no progress → back off one interval
                                      // Without a new gain, before next_retry → skip; at/after → attempt.
        assert!(!st.should_attempt(false, t0));
        assert!(st.should_attempt(false, t0 + AGENT_DISCOVERY_INTERVAL));
        // A new gain always overrides the backoff window.
        assert!(st.should_attempt(true, t0));
    }

    #[test]
    fn retry_state_progress_resets_and_empty_missing_clears() {
        let mut st = ConvergeRetryState::new();
        let t0 = Instant::now();
        st.record_attempt(false, t0);
        assert_eq!(st.backoff(), AGENT_DISCOVERY_INTERVAL);
        st.record_attempt(true, t0);
        assert_eq!(st.backoff(), Duration::ZERO);
        assert!(st.should_attempt(false, t0)); // nextRetry == now
        st.reset_backoff(); // go:83 — missing went empty mid-loop
        assert_eq!(st.backoff(), Duration::ZERO);
    }

    #[derive(Default)]
    struct FakeHost {
        stored: Mutex<Option<BTreeMap<String, AgentEntry>>>,
        startup: BTreeMap<String, AgentEntry>,
        skipped: Mutex<BTreeMap<String, String>>,
    }

    impl AgentsRefreshHost for FakeHost {
        fn stored_agents(&self) -> Option<BTreeMap<String, AgentEntry>> {
            self.stored.lock().unwrap().clone()
        }
        fn startup_agents(&self) -> BTreeMap<String, AgentEntry> {
            self.startup.clone()
        }
        fn publish_agents(&self, merged: BTreeMap<String, AgentEntry>) {
            *self.stored.lock().unwrap() = Some(merged);
        }
        fn set_skipped_agents(&self, skipped: BTreeMap<String, String>) {
            *self.skipped.lock().unwrap() = skipped;
        }
        fn skipped_agents_snapshot(&self) -> BTreeMap<String, String> {
            self.skipped.lock().unwrap().clone()
        }
    }

    #[test]
    fn agents_falls_back_to_startup_config_before_first_publish() {
        let mut host = FakeHost::default();
        host.startup.insert("codex".into(), entry("/fake/codex"));
        assert_eq!(host.agents().len(), 1);
        let mut published = BTreeMap::new();
        published.insert("claude".into(), entry("/fake/claude"));
        host.publish_agents(published);
        assert_eq!(host.agents().len(), 1);
        assert!(host.agents().contains_key("claude"));
    }

    #[test]
    fn merge_keeps_existing_entries_verbatim_and_adds_gained_only() {
        let mut current = BTreeMap::new();
        current.insert("codex".to_string(), entry("/pinned/codex"));
        let mut probed = current.clone();
        probed.insert("codex".to_string(), entry("/moved/codex")); // must NOT win
        probed.insert("antigravity".to_string(), entry("/fake/agy"));

        let merged = merge_discovered_agents(&current, &probed).unwrap();
        assert_eq!(merged["codex"].path, "/pinned/codex");
        assert_eq!(merged["antigravity"].path, "/fake/agy");
        assert_eq!(gained_providers(&current, &probed), vec!["antigravity"]);

        // Nothing new → no republish.
        assert!(merge_discovered_agents(&merged, &probed).is_none());
    }

    #[test]
    fn providers_missing_ignores_profiles_and_unknown_rows() {
        let (bid, builtin) = runtime("rt-1", "codex", "");
        let (pid, profiled) = runtime("rt-2", "claude", "prof-9");
        let (_uid, _gone) = runtime("rt-3", "qwen", "");
        let mut index = BTreeMap::new();
        index.insert(bid, builtin);
        index.insert(pid, profiled);

        let mut available = BTreeMap::new();
        available.insert("codex".to_string(), entry("/fake/codex"));
        available.insert("qwen".to_string(), entry("/fake/qwen"));

        // codex registered (built-in), claude only via profile → claude missing;
        // rt-3 is not in the index → counts as not registered, qwen missing too.
        let workspaces = vec![(
            "ws-1".to_string(),
            vec!["rt-1".to_string(), "rt-2".to_string(), "rt-3".to_string()],
        )];
        let missing = providers_missing_runtimes(&available, &workspaces, &index);
        assert_eq!(missing, vec!["qwen".to_string()]);
    }

    #[test]
    fn providers_missing_empty_when_no_workspaces_or_nothing_available() {
        let available = BTreeMap::new();
        let workspaces = vec![("ws-1".to_string(), vec!["rt-1".to_string()])];
        let index = BTreeMap::new();
        assert!(providers_missing_runtimes(&available, &workspaces, &index).is_empty());
        assert!(providers_missing_runtimes(&available, &[], &index).is_empty());
    }

    #[test]
    fn builtin_versions_skips_custom_profile_entries() {
        let mut builtin = HashMap::new();
        builtin.insert("type".to_string(), "codex".to_string());
        builtin.insert("version".to_string(), "1.2.3".to_string());
        let mut profiled = HashMap::new();
        profiled.insert("type".to_string(), "custom".to_string());
        profiled.insert("version".to_string(), "9.9.9".to_string());
        profiled.insert("profile_id".to_string(), "prof-1".to_string());

        let carried = builtin_versions_from_payload(&[builtin, profiled]);
        assert_eq!(carried.len(), 1);
        assert_eq!(carried["codex"], "1.2.3");
    }

    #[test]
    fn version_lag_flags_only_divergent_known_providers() {
        let mut carried = HashMap::new();
        carried.insert("codex".to_string(), "1.0.1".to_string());
        carried.insert("claude".to_string(), "2.0.0".to_string());

        let mut workspaces = BTreeMap::new();
        // ws-a: codex stale (transition), claude never registered (skipped —
        // converge's job), qwen extra record ignored.
        let mut ws_a = HashMap::new();
        ws_a.insert("codex".to_string(), "1.0.0".to_string());
        ws_a.insert("qwen".to_string(), "0.1.0".to_string());
        // ws-b: everything current → not behind.
        let mut ws_b = HashMap::new();
        ws_b.insert("codex".to_string(), "1.0.1".to_string());
        ws_b.insert("claude".to_string(), "2.0.0".to_string());
        // ws-c: codex registered with an EMPTY version → first-sighting
        // transition shape ("provider version", no "->").
        let mut ws_c = HashMap::new();
        ws_c.insert("codex".to_string(), String::new());
        workspaces.insert("ws-a".to_string(), ws_a);
        workspaces.insert("ws-b".to_string(), ws_b);
        workspaces.insert("ws-c".to_string(), ws_c);

        let (behind, transitions) = workspaces_behind_on_versions(&carried, &workspaces);
        assert_eq!(behind, vec!["ws-a".to_string(), "ws-c".to_string()]);
        assert_eq!(
            transitions,
            vec![
                "codex 1.0.0 -> 1.0.1".to_string(),
                "codex 1.0.1".to_string(),
            ]
        );
    }

    #[test]
    fn version_lag_skips_providers_with_no_record_for_the_workspace() {
        let mut carried = HashMap::new();
        carried.insert("codex".to_string(), "1.0.1".to_string());
        // No records at all: never registered here → converge's job, not a
        // version change (go:277–280).
        let workspaces = BTreeMap::from([("ws-x".to_string(), HashMap::new())]);
        let (behind, transitions) = workspaces_behind_on_versions(&carried, &workspaces);
        assert!(behind.is_empty());
        assert!(transitions.is_empty());
    }

    fn verdict(
        reason: &str,
        offline: Option<crate::client::RuntimeOfflineReason>,
    ) -> RuntimeVerdict {
        RuntimeVerdict {
            reason: reason.to_string(),
            offline,
        }
    }

    #[test]
    fn demotion_partition_keeps_profiles_and_uncondemned_rows() {
        let (bid, builtin_codex) = runtime("rt-1", "codex", "");
        let (pid, profiled) = runtime("rt-2", "codex", "prof-7"); // profile → keep
        let (kid, builtin_claude) = runtime("rt-3", "claude", ""); // not condemned → keep
        let mut index = BTreeMap::new();
        index.insert(bid.clone(), builtin_codex);
        index.insert(pid, profiled);
        index.insert(kid, builtin_claude);

        let mut workspaces = BTreeMap::new();
        workspaces.insert(
            "ws-1".to_string(),
            vec!["rt-1".to_string(), "rt-2".to_string(), "rt-3".to_string()],
        );

        let mut causes = BTreeMap::new();
        causes.insert(
            "codex".to_string(),
            verdict(
                "below minimum",
                Some(crate::client::RuntimeOfflineReason {
                    code: crate::client::RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE.into(),
                    detail: "exec format error".into(),
                    repair: None,
                }),
            ),
        );

        let (kept, part) = partition_demotable_runtimes(&workspaces, &index, &causes);
        assert_eq!(kept["ws-1"], vec!["rt-2".to_string(), "rt-3".to_string()]);
        assert_eq!(part.demoted_ids, vec!["rt-1".to_string()]);
        assert_eq!(part.demoted_by_workspace["ws-1"], vec!["rt-1".to_string()]);
        assert_eq!(part.demoted_providers["codex"], "below minimum");
        assert_eq!(part.offline_reasons.len(), 1);
        assert_eq!(
            part.offline_reasons["rt-1"].code,
            crate::client::RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE
        );
        assert_eq!(
            part.dropped_version_records,
            vec![("ws-1".to_string(), "codex".to_string())]
        );
    }

    #[test]
    fn demotion_partition_keeps_rows_missing_from_index() {
        let mut workspaces = BTreeMap::new();
        workspaces.insert("ws-1".to_string(), vec!["ghost".to_string()]);
        let index = BTreeMap::new();
        let causes = BTreeMap::new();
        let (kept, part) = partition_demotable_runtimes(&workspaces, &index, &causes);
        assert_eq!(kept["ws-1"], vec!["ghost".to_string()]);
        assert!(part.demoted_ids.is_empty());
    }

    #[test]
    fn revived_reasons_narrow_to_deregistered_rows() {
        let revived = RevivedRuntimes {
            ids: vec!["a".into(), "b".into()],
            reasons: HashMap::from([(
                "a".to_string(),
                crate::client::RuntimeOfflineReason {
                    code: crate::client::RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE.into(),
                    ..Default::default()
                },
            )]),
        };
        let narrowed = revived.reasons_for(&["a".into(), "c".into()]);
        assert_eq!(narrowed.len(), 1);
        assert!(narrowed.contains_key("a"));

        // Empty cause table → empty map (never attach a bare reason).
        assert!(RevivedRuntimes::default()
            .reasons_for(&["a".into()])
            .is_empty());
    }
}
