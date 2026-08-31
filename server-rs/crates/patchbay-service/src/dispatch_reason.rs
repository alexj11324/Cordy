//! Canonical execution-admission vocabulary — port of
//! Execution-admission vocabulary (PB-4525).
//!
//! A [`ReasonCode`] is decided at the branch that blocks/skips a run and
//! carried through to the response verbatim; it is never reverse-engineered
//! from a human-readable failure string. Codes are stable, localizable by
//! clients, and enumeration-safe: a code never reveals whether a private
//! agent exists, its name, or its owner.

use serde::{Deserialize, Serialize};

/// Stable, client-localizable admission/dispatch reason. The wire values are
/// the Go string constants, byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    /// Success paths.
    Queued,
    Coalesced,
    Deferred,

    /// The acting principal may not trigger this target under the
    /// invocation-permission model. Deliberately generic — it does not
    /// distinguish "target is private" from "target does not exist".
    InvocationNotAllowed,
    /// The target cannot run (archived agent, deleted / archived team,
    /// unresolvable leader, or no assignee).
    TargetUnavailable,
    /// The target is permitted and bound to a runtime, but that runtime is
    /// not online at dispatch time. The task is not lost — the user's fix is
    /// to bring the machine back, and queued work waits for it.
    RuntimeOffline,
    /// The target is bound to a runtime whose machine is reachable, but whose
    /// agent CLI cannot be executed there — the npm placeholder stub left
    /// behind when a package's postinstall was blocked is the case in the
    /// field (PB-6164). Distinct from [`ReasonCode::RuntimeOffline`] for the
    /// same reason agent_runtime_required is: waiting changes nothing here.
    /// The machine is already on, and the fix is a command the user runs on
    /// it, which the daemon reports with this verdict so clients can show it.
    RuntimeUnusable,
    /// The target is permitted but bound to no runtime at all
    /// (agent.runtime_id IS NULL), which is where an agent lands when its
    /// runtime is deleted (PB-5559). Distinct from
    /// [`ReasonCode::RuntimeOffline`] on purpose: there is no machine to
    /// bring back, nothing will ever claim work for this agent, and the only
    /// fix is binding it to a runtime. Clients that collapse the two send the
    /// user looking for an offline computer that does not exist.
    AgentRuntimeRequired,
    /// A fail-closed workspace could not resolve a responsible human for the
    /// run, so it was refused.
    AttributionBlocked,
    /// A run is already active/pending for this target and this trigger did
    /// not coalesce.
    AlreadyActive,
    /// The target was intentionally not (re-)triggered because doing so would
    /// be a self-trigger the guard suppresses, and no active run remains to
    /// cover it — e.g. a team leader's own @mention of its team whose
    /// latest task is already terminal. Not a permission block, but NOT
    /// success: nothing new runs. (Named to avoid implying the NEW comment
    /// was already processed.)
    SelfTriggerSuppressed,
    /// Policy-neutral refusal for an exhausted Cloud-provided automation
    /// interval.
    QuotaExceeded,
    /// An unexpected server error prevented a clean decision.
    InternalError,
}

impl ReasonCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasonCode::Queued => "queued",
            ReasonCode::Coalesced => "coalesced",
            ReasonCode::Deferred => "deferred",
            ReasonCode::InvocationNotAllowed => "invocation_not_allowed",
            ReasonCode::TargetUnavailable => "target_unavailable",
            ReasonCode::RuntimeOffline => "runtime_offline",
            ReasonCode::RuntimeUnusable => "runtime_unusable",
            ReasonCode::AgentRuntimeRequired => "agent_runtime_required",
            ReasonCode::AttributionBlocked => "attribution_blocked",
            ReasonCode::AlreadyActive => "already_active",
            ReasonCode::SelfTriggerSuppressed => "self_trigger_suppressed",
            ReasonCode::QuotaExceeded => "quota_exceeded",
            ReasonCode::InternalError => "internal_error",
        }
    }
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_match_go_constants() {
        assert_eq!(ReasonCode::Queued.as_str(), "queued");
        assert_eq!(ReasonCode::Coalesced.as_str(), "coalesced");
        assert_eq!(ReasonCode::Deferred.as_str(), "deferred");
        assert_eq!(
            ReasonCode::InvocationNotAllowed.as_str(),
            "invocation_not_allowed"
        );
        assert_eq!(ReasonCode::TargetUnavailable.as_str(), "target_unavailable");
        assert_eq!(ReasonCode::RuntimeOffline.as_str(), "runtime_offline");
        assert_eq!(ReasonCode::RuntimeUnusable.as_str(), "runtime_unusable");
        assert_eq!(
            ReasonCode::AgentRuntimeRequired.as_str(),
            "agent_runtime_required"
        );
        assert_eq!(
            ReasonCode::AttributionBlocked.as_str(),
            "attribution_blocked"
        );
        assert_eq!(ReasonCode::AlreadyActive.as_str(), "already_active");
        assert_eq!(
            ReasonCode::SelfTriggerSuppressed.as_str(),
            "self_trigger_suppressed"
        );
        assert_eq!(ReasonCode::QuotaExceeded.as_str(), "quota_exceeded");
        assert_eq!(ReasonCode::InternalError.as_str(), "internal_error");
    }

    #[test]
    fn serde_round_trips_through_the_wire_form() {
        let json = serde_json::to_string(&ReasonCode::RuntimeUnusable).unwrap();
        assert_eq!(json, r#""runtime_unusable""#);
        let back: ReasonCode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ReasonCode::RuntimeUnusable);
    }
}
