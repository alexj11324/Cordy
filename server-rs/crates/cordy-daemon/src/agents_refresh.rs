//! Agent discovery intervals and runtime demotion state shared by production
//! registration services.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::types::Runtime;

// ---------------------------------------------------------------------------
// Refresh intervals.
// ---------------------------------------------------------------------------

/// How often a running daemon re-checks
/// which agent CLIs are installed. A round is a handful of exec.LookPath
/// calls — the login-shell fallback is separately rate-limited by the much
/// longer shellResolveTTL — so this can be short enough that installing a CLI
/// feels immediate.
pub(crate) const AGENT_DISCOVERY_INTERVAL: Duration = Duration::from_secs(2 * 60);

/// How often a running daemon re-probes
/// the version of every agent CLI it already has registered, so an in-place
/// upgrade is picked up without a restart. Deliberately tracks
/// `selfReloadCheckInterval` rather than being pushed out on cost grounds: it
/// is also the window in which an unsupported CLI keeps claiming tasks.
pub(crate) const AGENT_VERSION_REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------------------
// Runtime demotion partition.
// ---------------------------------------------------------------------------

/// `runtimeVerdict` (daemon.go:2230–2233): the confirmed verdict a re-probe
/// reached about a provider's binary on disk. Construction (`newRuntimeVerdict`,
/// daemon.go:2237, incl. the ExecFormatRepair lookup) ports with
/// `detectBuiltinRuntimes` in daemon.go core; consumers only need these two
/// fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevivedRuntimes {
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

#[cfg(test)]
mod tests {
    use super::*;

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
