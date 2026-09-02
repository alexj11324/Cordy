package service

import (
	"encoding/json"
	"math"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type taskCapability struct {
	Action       string `json:"action"`
	ResourceType string `json:"resource_type"`
	ResourceID   string `json:"resource_id"`
}

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
	ProviderCredentialAction = "credential.use"
	ProviderIdentityResource = "provider_identity"
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
	for _, capability := range capabilities {
		if capability.Action != ProviderCredentialAction || capability.ResourceType != ProviderIdentityResource {
			continue
		}
		if capability.ResourceID == "*" || (runtime != "" && capability.ResourceID == runtime) {
			return true
		}
	}
	return false
}
