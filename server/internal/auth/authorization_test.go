package auth

import (
	"errors"
	"testing"
	"time"
)

func TestCapabilityScopeIsMonotonic(t *testing.T) {
	t.Parallel()
	parent := []Capability{{Action: ActionCredentialUse, ResourceType: ResourceProviderIdentity, ResourceID: "*"}}
	child := []Capability{{Action: ActionCredentialUse, ResourceType: ResourceProviderIdentity, ResourceID: "runtime-1"}}
	if !ScopeIsSubset(child, parent) {
		t.Fatal("exact child was not accepted under parent wildcard")
	}
	if ScopeIsSubset(parent, child) {
		t.Fatal("child wildcard widened an exact parent")
	}
	if ScopeCovers(child, ActionCredentialUse, ResourceProviderIdentity, "runtime-2") {
		t.Fatal("exact capability covered another resource")
	}
	if !ScopeCovers(parent, ActionCredentialUse, ResourceProviderIdentity, "runtime-2") {
		t.Fatal("parent wildcard did not cover runtime-2")
	}
	if ScopeCovers([]Capability{{Action: ActionTaskRead, ResourceType: ResourceTaskRun, ResourceID: "$task"}}, ActionTaskRead, ResourceTaskRun, "other-task") {
		t.Fatal("$task was treated as a wildcard")
	}
}

func TestValidateDelegationChainChecksFenceIdentityAndScope(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 9, 4, 12, 0, 0, 0, time.UTC)
	rootClaim := now.Add(-time.Minute)
	leafClaim := now.Add(-30 * time.Second)
	root := DelegationHop{
		ID: "root-lease", TaskID: "root-task", AgentID: "agent", WorkspaceID: "workspace",
		OnBehalfOfUserID: "user", DeviceID: "runtime", DelegationDepth: 0,
		DelegationFence: 11, Scope: []Capability{{Action: ActionCredentialUse, ResourceType: ResourceProviderIdentity, ResourceID: "*"}},
		ExpiresAt: now.Add(time.Hour), ClaimDispatchedAt: rootClaim, CurrentClaimDispatchedAt: rootClaim,
		CurrentTaskID: "root-task", CurrentAgentID: "agent", CurrentWorkspaceID: "workspace",
		CurrentOnBehalfOfUserID: "user", CurrentDeviceID: "runtime",
	}
	leaf := DelegationHop{
		ID: "leaf-lease", TaskID: "leaf-task", AgentID: "agent", WorkspaceID: "workspace",
		OnBehalfOfUserID: "user", DeviceID: "runtime", ParentID: root.ID, ParentFence: root.DelegationFence,
		DelegationFence: 12, DelegationDepth: 1,
		Scope: []Capability{{Action: ActionCredentialUse, ResourceType: ResourceProviderIdentity, ResourceID: "runtime"}},
		ExpiresAt: now.Add(time.Hour), ClaimDispatchedAt: leafClaim, CurrentClaimDispatchedAt: leafClaim,
		CurrentTaskID: "leaf-task", CurrentAgentID: "agent", CurrentWorkspaceID: "workspace",
		CurrentOnBehalfOfUserID: "user", CurrentDeviceID: "runtime",
	}
	request := AuthorizationContext{
		WorkspaceID: "workspace", TaskID: leaf.TaskID, LeaseID: leaf.ID,
		OnBehalfOfUserID: "user", ViaAgentID: "agent", DeviceID: "runtime",
	}
	if err := ValidateDelegationChain([]DelegationHop{leaf, root}, request, now); err != nil {
		t.Fatalf("valid chain rejected: %v", err)
	}

	badFence := leaf
	badFence.ParentFence++
	if !errors.Is(ValidateDelegationChain([]DelegationHop{badFence, root}, request, now), ErrAuthorizationLeaseMismatch) {
		t.Fatal("parent fence mismatch was accepted")
	}
	badScope := leaf
	badScope.Scope = []Capability{{Action: ActionRuntimeUse, ResourceType: ResourceRuntime, ResourceID: "runtime"}}
	if !errors.Is(ValidateDelegationChain([]DelegationHop{badScope, root}, request, now), ErrAuthorizationScopeWidened) {
		t.Fatal("child wildcard scope was accepted")
	}
	badIdentity := leaf
	badIdentity.CurrentDeviceID = "other-runtime"
	if !errors.Is(ValidateDelegationChain([]DelegationHop{badIdentity, root}, request, now), ErrAuthorizationLeaseMismatch) {
		t.Fatal("current device mismatch was accepted")
	}
}
