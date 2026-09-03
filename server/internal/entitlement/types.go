package entitlement

import (
	"context"
	"time"

	"github.com/google/uuid"
)

const SchemaVersion = 1

type GateName string

const (
	GateIssueCount    GateName = "issue_count"
	GateAutomationRuns GateName = "automation_runs"
	// GateImInstallationLimit caps how many installed channel connections a
	// managed workspace may hold. Unlike the counters above it is OPTIONAL on
	// the wire: Cloud deployments that have not rolled it out simply omit it,
	// and a missing gate resolves to an off decision (ReasonGateAbsent) rather
	// than failing the whole policy — issue-count and automation gating must
	// keep working on those deployments.
	GateImInstallationLimit GateName = "im_installation_limit"
	GateImAgentTurns        GateName = "im_agent_turns"
	GateHostedWorkspaceLimit GateName = "hosted_workspace_limit"
)

func (n GateName) valid() bool {
	switch n {
	case GateIssueCount, GateAutomationRuns, GateImInstallationLimit, GateImAgentTurns, GateHostedWorkspaceLimit:
		return true
	default:
		return false
	}
}

type Action string

const (
	ActionOff     Action = "off"
	ActionObserve Action = "observe"
	ActionEnforce Action = "enforce"
)

type Reason string

const (
	ReasonDisabled          Reason = "disabled"
	ReasonInvalidWorkspace  Reason = "invalid_workspace"
	ReasonUnknownGate       Reason = "unknown_gate"
	ReasonGateAbsent        Reason = "gate_absent"
	ReasonCacheFresh        Reason = "cache_fresh"
	ReasonRefreshed         Reason = "refreshed"
	ReasonStale             Reason = "stale"
	ReasonUnavailable       Reason = "unavailable"
	ReasonInvalidPolicy     Reason = "invalid_policy"
	ReasonVersionRegression Reason = "version_regression"
	ReasonStub              Reason = "stub"
)

// Gate is the effective instruction for one generic enforcement point. Limits
// and period boundaries come from Cloud; this package does not derive them.
type Gate struct {
	Action      Action
	Limit       *int
	PeriodStart *time.Time
	PeriodEnd   *time.Time
	ResetAt     *time.Time
}

// Decision carries enough source information for consumers to audit why a gate
// was enforced, observed, or disabled without exposing Cloud's private inputs.
type Decision struct {
	Gate                Gate
	Reason              Reason
	PolicyRevision      int64
	SubscriptionVersion int64
	CloudValidUntil     time.Time
}

// Provider is the only interface issue-count and automation consumers
// need. It deliberately has no error return: every failure is represented by a
// fail-open Decision with ActionOff (or ActionObserve for a bounded stale
// snapshot that can no longer enforce).
type Provider interface {
	Gate(ctx context.Context, workspaceID uuid.UUID, name GateName) Decision
}

// Observer is a low-cardinality instrumentation seam. Implementations must not
// attach workspace IDs or policy values as metric labels.
type Observer interface {
	RecordEntitlementCache(outcome string)
	RecordEntitlementRefresh(outcome string, durationSeconds float64)
	RecordEntitlementDecision(gate, action, reason string)
	RecordEntitlementVersionRegression()
}

func offDecision(reason Reason) Decision {
	return Decision{Gate: Gate{Action: ActionOff}, Reason: reason}
}

func cloneDecision(in Decision) Decision {
	in.Gate = cloneGate(in.Gate)
	return in
}

func cloneGate(in Gate) Gate {
	out := in
	if in.Limit != nil {
		value := *in.Limit
		out.Limit = &value
	}
	if in.PeriodStart != nil {
		value := *in.PeriodStart
		out.PeriodStart = &value
	}
	if in.PeriodEnd != nil {
		value := *in.PeriodEnd
		out.PeriodEnd = &value
	}
	if in.ResetAt != nil {
		value := *in.ResetAt
		out.ResetAt = &value
	}
	return out
}
