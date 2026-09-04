package channelquota

import (
	"context"
	"testing"
	"time"

	"github.com/google/uuid"

	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
)

type providerFunc func(context.Context, uuid.UUID, entitlement.GateName) entitlement.Decision

func (f providerFunc) Gate(ctx context.Context, workspaceID uuid.UUID, name entitlement.GateName) entitlement.Decision {
	return f(ctx, workspaceID, name)
}

func TestResolveHostedIMAdmission(t *testing.T) {
	workspaceID := uuid.New()
	if got := Resolve(context.Background(), nil, false, workspaceID); got.Kind != AdmissionBypass {
		t.Fatalf("self-host admission = %v, want bypass", got.Kind)
	}
	if got := Resolve(context.Background(), nil, true, workspaceID); got.Kind != AdmissionUnavailable {
		t.Fatalf("managed admission without provider = %v, want unavailable", got.Kind)
	}

	start := time.Now().UTC().Truncate(time.Second)
	end := start.Add(24 * time.Hour)
	reset := end.Add(time.Hour)
	limit := 100
	provider := providerFunc(func(_ context.Context, gotWorkspace uuid.UUID, name entitlement.GateName) entitlement.Decision {
		if gotWorkspace != workspaceID || name != entitlement.GateImAgentTurns {
			t.Fatalf("gate request = %s %s", gotWorkspace, name)
		}
		return entitlement.Decision{Gate: entitlement.Gate{
			Action: entitlement.ActionEnforce, Limit: &limit,
			PeriodStart: &start, PeriodEnd: &end, ResetAt: &reset,
		}}
	})
	got := Resolve(context.Background(), provider, true, workspaceID)
	if got.Kind != AdmissionLimited || got.Window.Limit != 100 || got.Window.PeriodStart != start || got.Window.ResetAt != reset {
		t.Fatalf("managed admission = %+v", got)
	}
}

func TestResolveObserveBypassesAndMalformedEnforceFailsClosed(t *testing.T) {
	workspaceID := uuid.New()
	limit := 1
	observe := providerFunc(func(context.Context, uuid.UUID, entitlement.GateName) entitlement.Decision {
		return entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionObserve, Limit: &limit}}
	})
	if got := Resolve(context.Background(), observe, true, workspaceID); got.Kind != AdmissionBypass {
		t.Fatalf("observe admission = %v, want bypass", got.Kind)
	}
	malformed := providerFunc(func(context.Context, uuid.UUID, entitlement.GateName) entitlement.Decision {
		return entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionEnforce, Limit: &limit}}
	})
	if got := Resolve(context.Background(), malformed, true, workspaceID); got.Kind != AdmissionUnavailable {
		t.Fatalf("malformed admission = %v, want unavailable", got.Kind)
	}
}
