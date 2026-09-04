package auth

import (
	"errors"
	"fmt"
	"time"
)

// AuthorizationPolicyVersion is stamped on every decision written by the Go
// authorization control plane. Keep this separate from the JWT/token format:
// changing a policy must make old explanations identifiable without invalidating
// unrelated authentication credentials.
const AuthorizationPolicyVersion = "phase1-go-2026-09-03"

const MaxDelegationDepth = 8

const (
	PrincipalUser            = "user"
	PrincipalTeam            = "team"
	PrincipalAgentDefinition = "agent_definition"
	PrincipalTaskRun         = "task_run"
	PrincipalDeviceRuntime   = "device_runtime"
	PrincipalService         = "service"
	PrincipalSystem          = "system"
)

const (
	EffectAllow          = "allow"
	EffectDeny           = "deny"
	EffectRequireApproval = "require_approval"
)

const (
	ActionAgentInvoke     = "agent.invoke"
	ActionCredentialUse   = "credential.use"
	ActionCredentialRead  = "credential.read_secret"
	ActionRuntimeRead     = "runtime.read"
	ActionRuntimeUpdate   = "runtime.update"
	ActionRuntimeUse      = "runtime.use"
	ActionTaskRead        = "task.read"
	ActionTaskUpdate      = "task.update"
	ActionResourceRead    = "resource.read"
	ActionResourceUse     = "resource.use"
	ActionWorkspaceManage = "workspace.manage"
)

const (
	ResourceProviderIdentity = "provider_identity"
	ResourceTaskRun          = "task_run"
	ResourceProject          = "project_resource"
	ResourceRuntime          = "runtime"
)

var (
	ErrAuthorizationWorkspaceRequired = errors.New("authorization workspace is required")
	ErrAuthorizationDelegationDepth  = errors.New("authorization delegation depth exceeded")
	ErrAuthorizationDelegationCycle  = errors.New("authorization delegation cycle detected")
	ErrAuthorizationLeaseInvalid     = errors.New("authorization lease is invalid")
	ErrAuthorizationLeaseMismatch    = errors.New("authorization lease identity mismatch")
	ErrAuthorizationScopeWidened      = errors.New("authorization delegation widened capability scope")
)

// Effect names are persisted in authorization_grant and authorization_audit_event.
// This helper is deliberately strict so a future database row cannot silently
// become an allow because an unknown value was treated as the zero value.
func ValidEffect(effect string) bool {
	return effect == EffectAllow || effect == EffectDeny || effect == EffectRequireApproval
}

// Capability is the transport-independent shape of a task lease capability.
// ResourceID "*" is the only wildcard. "$task" is a literal task-scoped id,
// not a wildcard, and therefore cannot cover another task.
type Capability struct {
	Action       string `json:"action"`
	ResourceType string `json:"resource_type"`
	ResourceID   string `json:"resource_id"`
}

func (c Capability) Covers(action, resourceType, resourceID string) bool {
	if c.Action == "" || c.ResourceType == "" || c.ResourceID == "" {
		return false
	}
	if c.Action != action || c.ResourceType != resourceType {
		return false
	}
	return c.ResourceID == "*" || c.ResourceID == resourceID
}

func ScopeCovers(scope []Capability, action, resourceType, resourceID string) bool {
	for _, capability := range scope {
		if capability.Covers(action, resourceType, resourceID) {
			return true
		}
	}
	return false
}

// ScopeIsSubset implements the monotonic delegation rule. A child can copy an
// exact capability or narrow a parent wildcard, but it cannot introduce an
// action/resource pair or turn an exact resource into a wildcard.
func ScopeIsSubset(child, parent []Capability) bool {
	for _, requested := range child {
		covered := false
		for _, bound := range parent {
			if bound.Action != requested.Action || bound.ResourceType != requested.ResourceType {
				continue
			}
			if bound.ResourceID == "*" || bound.ResourceID == requested.ResourceID {
				covered = requested.ResourceID != ""
				break
			}
		}
		if !covered {
			return false
		}
	}
	return true
}

// AuthorizationContext identifies the server-stamped actor and scope used by
// an authorization request. IDs are strings here so the foundation stays
// independent of PostgreSQL/transport packages; adapters convert database UUIDs
// at the service boundary.
type AuthorizationContext struct {
	WorkspaceID      string
	WorkspaceRole    string
	OnBehalfOfUserID string
	ViaAgentID       string
	DeviceID         string
	TaskID           string
	LeaseID          string
}

type AuthorizationRequest struct {
	PrincipalType string
	PrincipalID   string
	Action        string
	ResourceType  string
	ResourceID    string
	Context       AuthorizationContext
}

// DelegationHop is a database adapter shape for validating a recursive lease
// chain. Hops are ordered leaf first, matching the SQL recursive queries.
type DelegationHop struct {
	ID                       string
	TaskID                   string
	AgentID                  string
	WorkspaceID              string
	OnBehalfOfUserID         string
	DeviceID                 string
	ParentID                 string
	ParentFence              int64
	DelegationFence          int64
	DelegationDepth          int
	Scope                    []Capability
	Revoked                  bool
	ExpiresAt                time.Time
	ClaimDispatchedAt        time.Time
	CurrentClaimDispatchedAt time.Time
	CurrentTaskID            string
	CurrentAgentID           string
	CurrentWorkspaceID       string
	CurrentOnBehalfOfUserID  string
	CurrentDeviceID          string
	CurrentAgentArchived     bool
}

// ValidateDelegationChain is the pure invariant checker used by lease adapters
// and tests. SQL still performs the same checks close to the row read; keeping
// the invariants here prevents a future non-Postgres consumer from weakening
// scope/fence/identity semantics.
func ValidateDelegationChain(chain []DelegationHop, request AuthorizationContext, now time.Time) error {
	if request.WorkspaceID == "" {
		return ErrAuthorizationWorkspaceRequired
	}
	if len(chain) == 0 || len(chain) > MaxDelegationDepth+1 {
		return ErrAuthorizationDelegationDepth
	}
	seen := make(map[string]struct{}, len(chain))
	for index, hop := range chain {
		if hop.ID == "" {
			return ErrAuthorizationLeaseInvalid
		}
		if _, exists := seen[hop.ID]; exists {
			return ErrAuthorizationDelegationCycle
		}
		seen[hop.ID] = struct{}{}
		if hop.DelegationDepth < 0 || hop.DelegationDepth > MaxDelegationDepth {
			return ErrAuthorizationDelegationDepth
		}
		if !hop.ExpiresAt.After(now) || hop.Revoked || hop.CurrentAgentArchived {
			return ErrAuthorizationLeaseInvalid
		}
		if !hop.ClaimDispatchedAt.Equal(hop.CurrentClaimDispatchedAt) ||
			hop.WorkspaceID != hop.CurrentWorkspaceID ||
			hop.AgentID != hop.CurrentAgentID ||
			hop.OnBehalfOfUserID != hop.CurrentOnBehalfOfUserID ||
			hop.DeviceID != hop.CurrentDeviceID {
			return ErrAuthorizationLeaseMismatch
		}
		if hop.WorkspaceID != request.WorkspaceID || hop.AgentID == "" || hop.DeviceID == "" {
			return ErrAuthorizationLeaseMismatch
		}
		if index == 0 {
			if request.TaskID != "" && hop.TaskID != request.TaskID {
				return ErrAuthorizationLeaseMismatch
			}
			if request.LeaseID != "" && hop.ID != request.LeaseID {
				return ErrAuthorizationLeaseMismatch
			}
			if request.OnBehalfOfUserID != "" && hop.OnBehalfOfUserID != request.OnBehalfOfUserID {
				return ErrAuthorizationLeaseMismatch
			}
			if request.ViaAgentID != "" && hop.AgentID != request.ViaAgentID {
				return ErrAuthorizationLeaseMismatch
			}
			if request.DeviceID != "" && hop.DeviceID != request.DeviceID {
				return ErrAuthorizationLeaseMismatch
			}
		}
		if index == len(chain)-1 {
			if hop.ParentID != "" || hop.DelegationDepth != 0 {
				return ErrAuthorizationDelegationDepth
			}
			continue
		}
		parent := chain[index+1]
		if hop.ParentID != parent.ID || hop.DelegationDepth != parent.DelegationDepth+1 || hop.ParentFence != parent.DelegationFence {
			return ErrAuthorizationLeaseMismatch
		}
		if hop.WorkspaceID != parent.WorkspaceID || hop.AgentID != parent.AgentID ||
			hop.OnBehalfOfUserID != parent.OnBehalfOfUserID || hop.DeviceID != parent.DeviceID {
			return ErrAuthorizationLeaseMismatch
		}
		if !ScopeIsSubset(hop.Scope, parent.Scope) {
			return fmt.Errorf("%w at lease %s", ErrAuthorizationScopeWidened, hop.ID)
		}
	}
	return nil
}
