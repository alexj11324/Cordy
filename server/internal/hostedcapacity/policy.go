package hostedcapacity

import (
	"context"
	"errors"
	"fmt"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// ErrLimitReached is the admission refusal when a workspace already holds its
// cap of installed connections (and the new install is not reconnecting an
// existing slot). Handlers render it as 403 im_installation_limit_reached.
var ErrLimitReached = errors.New("hosted messaging installation limit reached")

// ErrQuotaUnavailable is the fail-closed refusal when the capacity feature is
// enabled but the Cloud policy cannot be trusted (unreachable, gate absent on
// this Cloud, invalid, or expired past its stale grace). Handlers render it as
// 503 im_installation_quota_unavailable. It never means "over limit".
var ErrQuotaUnavailable = errors.New("hosted messaging installation quota is temporarily unavailable")

type policyKind uint8

const (
	// policyDisabled: the feature is off (self-hosted or not enabled). No
	// check, no reconcile, no pause markers — the product behaves as if the
	// package did not exist.
	policyDisabled policyKind = iota
	// policyUnavailable: enabled but the Cloud policy is not trustworthy.
	// Admission fails closed; the reconciler preserves the last authoritative
	// pause state instead of guessing.
	policyUnavailable
	// policyBypass: the policy is in observe (stale-downgraded) mode — track
	// nothing, cap nothing.
	policyBypass
	// policyUnlimited: enforce with a null limit (Pro). No cap, but reconcile
	// still clears any pause markers left by a downgrade.
	policyUnlimited
	// policyLimited: enforce with a concrete limit.
	policyLimited
)

// Policy is the resolved capacity instruction for one workspace. The zero
// value is disabled.
type Policy struct {
	kind  policyKind
	limit int64
}

// Limit reports the effective cap. A nil limit means "no admission check":
// either the policy is not limited, or the feature is disabled/bypassed.
func (p Policy) Limit() *int64 {
	if p.kind != policyLimited {
		return nil
	}
	value := p.limit
	return &value
}

// Unavailable reports whether the policy failed closed. Distinct from
// disabled: callers answer 503 on unavailable and skip on disabled.
func (p Policy) Unavailable() bool { return p.kind == policyUnavailable }

// Disabled reports whether the feature is entirely off for this deployment.
func (p Policy) Disabled() bool { return p.kind == policyDisabled }

// Resolver turns the entitlement gate decision into a capacity policy. A nil
// provider with the feature enabled resolves unavailable (fail closed): an
// operator who turned the feature on owes it a trustworthy policy source.
type Resolver struct {
	enabled  bool
	provider entitlement.Provider
}

// NewResolver builds the resolver. enabled comes from the deployment switch
// (PATCHBAY_HOSTED_IM_CAPACITY); provider is the shared entitlement client,
// which is only connected when PATCHBAY_CLOUD_URL is set.
func NewResolver(enabled bool, provider entitlement.Provider) *Resolver {
	return &Resolver{enabled: enabled, provider: provider}
}

// Enabled reports whether the deployment runs the capacity feature at all.
func (r *Resolver) Enabled() bool { return r != nil && r.enabled }

// Resolve maps one entitlement decision onto the capacity policy. The
// entitlement package fails OPEN by design (stale policy never blocks issue
// creation); this is the fail-CLOSED seam for hosted capacity — every off
// decision, whatever its reason (disabled client, absent gate, unreachable
// Cloud, invalid policy), means "cannot trust the cap" and refuses admission.
func (r *Resolver) Resolve(ctx context.Context, workspaceID pgtype.UUID) Policy {
	if !r.Enabled() {
		return Policy{kind: policyDisabled}
	}
	if r.provider == nil {
		return Policy{kind: policyUnavailable}
	}
	decision := r.provider.Gate(ctx, workspaceUUID(workspaceID), entitlement.GateImInstallationLimit)
	switch decision.Gate.Action {
	case entitlement.ActionObserve:
		return Policy{kind: policyBypass}
	case entitlement.ActionEnforce:
		if decision.Gate.Limit == nil {
			return Policy{kind: policyUnlimited}
		}
		if *decision.Gate.Limit < 0 {
			return Policy{kind: policyUnavailable}
		}
		return Policy{kind: policyLimited, limit: int64(*decision.Gate.Limit)}
	default:
		return Policy{kind: policyUnavailable}
	}
}

func workspaceUUID(id pgtype.UUID) uuid.UUID {
	return uuid.UUID(id.Bytes)
}

// AdmitQueries is the transaction-bound slice of generated queries admission
// needs. Satisfied by the generated *db.Queries (or its WithTx binding); each
// integration's private queries interface declares these two methods so its
// tx-bound value can be passed straight through.
type AdmitQueries interface {
	LockWorkspaceForHostedCapacity(ctx context.Context, workspaceID pgtype.UUID) (pgtype.UUID, error)
	ChannelInstallationCapacitySnapshot(ctx context.Context, arg db.ChannelInstallationCapacitySnapshotParams) (db.ChannelInstallationCapacitySnapshotRow, error)
}

// AdmitInstall enforces the cap inside the caller's install transaction, which
// must already be open: the workspace row lock taken here serializes the
// count against concurrent installs, and the surrounding COMMIT releases it
// only after the upsert has claimed its slot. A nil limit is a no-op, so
// disabled/bypass/unlimited deployments run exactly the pre-feature code
// path. A missing workspace refuses (the install would fail anyway); a
// same-slot reconnect is always allowed so a workspace can never be locked
// out of managing an installation that already counts against its cap.
func AdmitInstall(ctx context.Context, q AdmitQueries, workspaceID pgtype.UUID, channelType string, agentID pgtype.UUID, limit *int64) error {
	if limit == nil {
		return nil
	}
	if _, err := q.LockWorkspaceForHostedCapacity(ctx, workspaceID); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return ErrLimitReached
		}
		return fmt.Errorf("lock workspace for hosted capacity: %w", err)
	}
	snapshot, err := q.ChannelInstallationCapacitySnapshot(ctx, db.ChannelInstallationCapacitySnapshotParams{
		WorkspaceID: workspaceID,
		ChannelType: channelType,
		AgentID:     agentID,
	})
	if err != nil {
		return fmt.Errorf("read hosted capacity snapshot: %w", err)
	}
	if snapshot.InstalledCount < *limit || snapshot.SameSlot {
		return nil
	}
	return ErrLimitReached
}
