package service

import (
	"encoding/json"
	"errors"
	"math"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

var (
	// ErrCapabilityLeaseAlreadyFinalized is returned when the claim fence's
	// unique slot already has a lease. Replaying the same daemon response must
	// not mint a second bearer or duplicate its delivery receipt.
	ErrCapabilityLeaseAlreadyFinalized = errors.New("capability lease already finalized")
	// ErrCapabilityLeaseIssuanceDenied means the claim/parent/agent/workspace
	// invariants rejected issuance. It is distinct from replay so callers can
	// requeue/cancel without treating a malformed delegation as success.
	ErrCapabilityLeaseIssuanceDenied = errors.New("capability lease issuance denied")
)

// Keep the historical private name as an alias so existing service tests and
// callers cannot accidentally define a second, subtly different scope shape.
type taskCapability = auth.Capability

// RootTaskCapabilityScope is a server-owned ceiling. Resource ACLs and the
// provider authorizer narrow it at use time; delegated claims are intersected
// with their parent's effective scope by CreateTaskToken.
func RootTaskCapabilityScope(task db.AgentTaskQueue) []byte {
	scope := []taskCapability{
		{Action: "task.read", ResourceType: "task_run", ResourceID: "$task"},
		{Action: "task.update", ResourceType: "task_run", ResourceID: "$task"},
		{Action: "agent.invoke", ResourceType: "agent_definition", ResourceID: "*"},
		{Action: "resource.read", ResourceType: "project_resource", ResourceID: "*"},
		{Action: "resource.use", ResourceType: "project_resource", ResourceID: "*"},
	}
	if task.RuntimeID.Valid {
		resourceID := uuidText(task.RuntimeID)
		scope = append(scope,
			taskCapability{Action: "runtime.use", ResourceType: "runtime", ResourceID: resourceID},
			taskCapability{Action: "credential.use", ResourceType: "provider_identity", ResourceID: resourceID},
		)
	}
	raw, err := json.Marshal(scope)
	if err != nil {
		return []byte(`[]`)
	}
	return raw
}

// TaskClaimFence deterministically binds one lease to one dispatched claim.
// A replay computes the same value and loses the unique claim index; a
// re-dispatch changes dispatched_at and therefore invalidates the old lease.
func TaskClaimFence(task db.AgentTaskQueue) int64 {
	if !task.DispatchedAt.Valid {
		return int64(task.Attempt)
	}
	micros := task.DispatchedAt.Time.UnixMicro()
	if micros > math.MaxInt64/32 {
		micros = math.MaxInt64 / 32
	}
	if micros < math.MinInt64/32 {
		micros = math.MinInt64 / 32
	}
	base := micros * 32
	if task.Attempt > 0 && base > math.MaxInt64-int64(task.Attempt) {
		return math.MaxInt64
	}
	return base + int64(task.Attempt)
}

// ProviderCredentialAction and ProviderIdentityResource name the one capability
// a provider operation needs. They are the same strings RootTaskCapabilityScope
// mints, so a lease minted for one runtime cannot spend another runtime's
// provider identity.
const (
	ProviderCredentialAction = auth.ActionCredentialUse
	ProviderIdentityResource = auth.ResourceProviderIdentity
)

// LeaseAuthorizesProviderUse decides whether one structurally valid lease may
// be spent on a provider operation for exactly this task, agent, workspace,
// runtime and human.
//
// GetValidTaskCapabilityLease has already walked the delegation chain for
// revocation, expiry, task state and the claim fence. What it deliberately does
// not do is bind the row to the operation being attempted: the caller names the
// task/agent/runtime it claims to be acting for, and a lease for a *different*
// one is still a perfectly valid lease. Comparing here is what stops a daemon
// holding one live lease from spending it against another task's provider
// identity.
func LeaseAuthorizesProviderUse(lease db.GetValidTaskCapabilityLeaseRow, taskID, agentID, workspaceID, runtimeID, onBehalfOfUserID pgtype.UUID) bool {
	if lease.RevokedAt.Valid || !lease.ExpiresAt.Valid || !lease.ExpiresAt.Time.After(time.Now()) {
		return false
	}
	if lease.TaskID != taskID || lease.AgentID != agentID || lease.WorkspaceID != workspaceID {
		return false
	}
	if lease.DeviceID != runtimeID || lease.OnBehalfOfUserID != onBehalfOfUserID {
		return false
	}
	return scopeGrantsProviderCredential(lease.Scope, runtimeID)
}

func scopeGrantsProviderCredential(scope []byte, runtimeID pgtype.UUID) bool {
	var capabilities []taskCapability
	if json.Unmarshal(scope, &capabilities) != nil {
		return false
	}
	runtime := uuidText(runtimeID)
	return auth.ScopeCovers(capabilities, ProviderCredentialAction, ProviderIdentityResource, runtime)
}

// TaskLeaseAllows is the shared task-token gate for non-provider consumers.
// A task token is an authority ceiling, not an actor label: every task-scoped
// mutation that reaches this helper must still name an action/resource pair the
// issued scope covers. "*" is the only resource wildcard; "$task" remains an
// exact task resource.
func TaskLeaseAllows(scope []byte, action, resourceType, resourceID string) bool {
	var capabilities []auth.Capability
	if len(scope) == 0 || json.Unmarshal(scope, &capabilities) != nil {
		return false
	}
	return auth.ScopeCovers(capabilities, action, resourceType, resourceID)
}
