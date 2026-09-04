package service

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func leaseFixtureTask(at time.Time, attempt int32, runtimeID pgtype.UUID) db.AgentTaskQueue {
	return db.AgentTaskQueue{
		DispatchedAt: pgtype.Timestamptz{Time: at, Valid: true},
		Attempt:      attempt,
		RuntimeID:    runtimeID,
	}
}

// TestTaskClaimFenceIsDeterministicPerClaim covers the property the fence
// exists for: it identifies a claim, not a moment. A daemon that replays the
// same claim computes the same fence and loses the unique claim index, while a
// re-dispatch or a retry computes a different one — which is what makes the
// lease from the previous attempt distinguishable from the current one rather
// than merely older.
func TestTaskClaimFenceIsDeterministicPerClaim(t *testing.T) {
	t.Parallel()

	runtimeID := testUUID(0x11)
	dispatched := time.Date(2026, 9, 2, 12, 0, 0, 0, time.UTC)
	claim := leaseFixtureTask(dispatched, 0, runtimeID)

	if TaskClaimFence(claim) != TaskClaimFence(claim) {
		t.Fatalf("replayed claim produced two different fences")
	}
	if retry := TaskClaimFence(leaseFixtureTask(dispatched, 1, runtimeID)); retry == TaskClaimFence(claim) {
		t.Fatalf("attempt 1 reused attempt 0's fence %d", retry)
	}
	redispatched := TaskClaimFence(leaseFixtureTask(dispatched.Add(time.Millisecond), 0, runtimeID))
	if redispatched == TaskClaimFence(claim) {
		t.Fatalf("re-dispatch reused the previous claim's fence %d", redispatched)
	}
	if redispatched <= TaskClaimFence(claim) {
		t.Fatalf("fence went backwards on re-dispatch: %d then %d", TaskClaimFence(claim), redispatched)
	}
}

// TestRootTaskCapabilityScopeBindsProviderUseToItsRuntime asserts a root lease
// carries provider credential use for exactly the runtime the task runs on, and
// that a task with no runtime carries none at all: an unbound task must not
// mint a lease that any runtime's provider identity would satisfy.
func TestRootTaskCapabilityScopeBindsProviderUseToItsRuntime(t *testing.T) {
	t.Parallel()

	runtimeID := testUUID(0x22)
	scope := RootTaskCapabilityScope(leaseFixtureTask(time.Now(), 0, runtimeID))
	var capabilities []taskCapability
	if err := json.Unmarshal(scope, &capabilities); err != nil {
		t.Fatalf("decode scope: %v", err)
	}
	var provider []taskCapability
	for _, capability := range capabilities {
		if capability.Action == ProviderCredentialAction && capability.ResourceType == ProviderIdentityResource {
			provider = append(provider, capability)
		}
	}
	if len(provider) != 1 || provider[0].ResourceID != uuidText(runtimeID) {
		t.Fatalf("provider capabilities = %#v, want exactly one bound to %s", provider, uuidText(runtimeID))
	}

	unbound := RootTaskCapabilityScope(db.AgentTaskQueue{})
	if !scopeGrantsProviderCredential(scope, runtimeID) {
		t.Fatalf("runtime-bound scope does not authorize its own runtime")
	}
	if scopeGrantsProviderCredential(unbound, runtimeID) {
		t.Fatalf("a task with no runtime minted a scope that authorizes one")
	}
	if scopeGrantsProviderCredential(scope, testUUID(0x33)) {
		t.Fatalf("scope for one runtime authorized another runtime's provider identity")
	}
}

// TestLeaseAuthorizesProviderUseRequiresExactBinding covers the check that the
// chain walk deliberately leaves to Go: a lease is only spendable on the
// operation it was minted for. Every field here names a different live lease
// the same daemon could be holding at the same moment.
func TestLeaseAuthorizesProviderUseRequiresExactBinding(t *testing.T) {
	t.Parallel()

	taskID, agentID, workspaceID, runtimeID, userID := testUUID(1), testUUID(2), testUUID(3), testUUID(4), testUUID(5)
	valid := db.GetValidTaskCapabilityLeaseRow{
		TaskID:           taskID,
		AgentID:          agentID,
		WorkspaceID:      workspaceID,
		DeviceID:         runtimeID,
		OnBehalfOfUserID: userID,
		ExpiresAt:        pgtype.Timestamptz{Time: time.Now().Add(time.Hour), Valid: true},
		Scope:            RootTaskCapabilityScope(leaseFixtureTask(time.Now(), 0, runtimeID)),
	}
	if !LeaseAuthorizesProviderUse(valid, taskID, agentID, workspaceID, runtimeID, userID) {
		t.Fatalf("a live, correctly bound lease was refused")
	}

	other := testUUID(9)
	for name, lease := range map[string]db.GetValidTaskCapabilityLeaseRow{
		"another task's lease":  withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) { l.TaskID = other }),
		"another agent's lease": withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) { l.AgentID = other }),
		"another workspace":     withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) { l.WorkspaceID = other }),
		"another runtime":       withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) { l.DeviceID = other }),
		"another human":         withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) { l.OnBehalfOfUserID = other }),
		"revoked": withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) {
			l.RevokedAt = pgtype.Timestamptz{Time: time.Now(), Valid: true}
		}),
		"expired": withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) {
			l.ExpiresAt = pgtype.Timestamptz{Time: time.Now().Add(-time.Second), Valid: true}
		}),
		"no provider capability": withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) {
			l.Scope = []byte(`[{"action":"agent.invoke","resource_type":"agent_definition","resource_id":"*"}]`)
		}),
		"provider use elsewhere": withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) {
			l.Scope = []byte(`[{"action":"credential.use","resource_type":"provider_identity","resource_id":"` + uuidText(other) + `"}]`)
		}),
		"unreadable scope": withLease(valid, func(l *db.GetValidTaskCapabilityLeaseRow) { l.Scope = []byte(`not json`) }),
	} {
		if LeaseAuthorizesProviderUse(lease, taskID, agentID, workspaceID, runtimeID, userID) {
			t.Errorf("%s authorized a provider operation", name)
		}
	}
}

func TestTaskLeaseAllowsKeepsTaskAndResourceScopesExact(t *testing.T) {
	t.Parallel()
	if !TaskLeaseAllows([]byte(`[{"action":"resource.use","resource_type":"project_resource","resource_id":"*"}]`), "resource.use", "project_resource", "project-1") {
		t.Fatal("wildcard project capability did not cover a project")
	}
	if TaskLeaseAllows([]byte(`[{"action":"task.read","resource_type":"task_run","resource_id":"$task"}]`), "task.read", "task_run", "another-task") {
		t.Fatal("$task capability covered another task")
	}
	if TaskLeaseAllows([]byte(`[{"action":"resource.use","resource_type":"project_resource","resource_id":"project-1"}]`), "resource.use", "project_resource", "project-2") {
		t.Fatal("exact project capability covered another project")
	}
	if TaskLeaseAllows([]byte(`not-json`), "resource.use", "project_resource", "project-1") {
		t.Fatal("malformed scope was accepted")
	}
}

func withLease(base db.GetValidTaskCapabilityLeaseRow, mutate func(*db.GetValidTaskCapabilityLeaseRow)) db.GetValidTaskCapabilityLeaseRow {
	mutate(&base)
	return base
}
