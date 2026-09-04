package hostedcapacity

import (
	"context"
	"errors"
	"testing"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/entitlement"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type stubProvider struct {
	decision entitlement.Decision
}

func (s stubProvider) Gate(context.Context, uuid.UUID, entitlement.GateName) entitlement.Decision {
	return s.decision
}

func limitPtr(v int) *int { return &v }

func TestResolveMapsEntitlementOntoCapacityPolicy(t *testing.T) {
	ws := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	tests := []struct {
		name       string
		resolver   *Resolver
		decision   entitlement.Decision
		wantKind   policyKind
		wantLimit  *int64
	}{
		{
			name:     "disabled resolver stays disabled",
			resolver: NewResolver(false, stubProvider{}),
			wantKind: policyDisabled,
		},
		{
			name:     "nil resolver stays disabled",
			resolver: nil,
			wantKind: policyDisabled,
		},
		{
			name:     "enabled without provider fails closed",
			resolver: NewResolver(true, nil),
			wantKind: policyUnavailable,
		},
		{
			// The fail-closed seam: every off decision — unreachable Cloud,
			// absent gate, disabled client, invalid policy — refuses.
			name:     "off decision is unavailable",
			resolver: NewResolver(true, stubProvider{}),
			decision: entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionOff}},
			wantKind: policyUnavailable,
		},
		{
			name:     "observe decision is bypass",
			resolver: NewResolver(true, stubProvider{}),
			decision: entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionObserve}},
			wantKind: policyBypass,
		},
		{
			name:      "enforce with null limit is unlimited",
			resolver:  NewResolver(true, stubProvider{}),
			decision:  entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionEnforce}},
			wantKind:  policyUnlimited,
			wantLimit: nil,
		},
		{
			name:      "enforce with limit is limited",
			resolver:  NewResolver(true, stubProvider{}),
			decision:  entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionEnforce, Limit: limitPtr(4)}},
			wantKind:  policyLimited,
			wantLimit: ptrOf(int64(4)),
		},
		{
			name:     "negative limit is refused, not clamped",
			resolver: NewResolver(true, stubProvider{}),
			decision: entitlement.Decision{Gate: entitlement.Gate{Action: entitlement.ActionEnforce, Limit: limitPtr(-1)}},
			wantKind: policyUnavailable,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			provider := stubProvider{decision: tt.decision}
			if tt.resolver != nil {
				tt.resolver = NewResolver(tt.resolver.enabled, provider)
			}
			got := tt.resolver.Resolve(context.Background(), ws)
			if got.kind != tt.wantKind {
				t.Fatalf("kind = %v, want %v", got.kind, tt.wantKind)
			}
			gotLimit := got.Limit()
			if (gotLimit == nil) != (tt.wantLimit == nil) {
				t.Fatalf("limit = %v, want %v", gotLimit, tt.wantLimit)
			}
			if gotLimit != nil && *gotLimit != *tt.wantLimit {
				t.Fatalf("limit = %d, want %d", *gotLimit, *tt.wantLimit)
			}
		})
	}
}

func ptrOf(v int64) *int64 { return &v }

type fakeAdmitQueries struct {
	workspaceMissing bool
	snapshot         db.ChannelInstallationCapacitySnapshotRow
	snapshotErr      error
	locked           bool
}

func (f *fakeAdmitQueries) LockWorkspaceForHostedCapacity(context.Context, pgtype.UUID) (pgtype.UUID, error) {
	if f.workspaceMissing {
		return pgtype.UUID{}, pgx.ErrNoRows
	}
	f.locked = true
	return pgtype.UUID{Bytes: [16]byte{1}, Valid: true}, nil
}

func (f *fakeAdmitQueries) ChannelInstallationCapacitySnapshot(context.Context, db.ChannelInstallationCapacitySnapshotParams) (db.ChannelInstallationCapacitySnapshotRow, error) {
	return f.snapshot, f.snapshotErr
}

func TestAdmitInstall(t *testing.T) {
	ws := pgtype.UUID{Bytes: [16]byte{1}, Valid: true}
	agent := pgtype.UUID{Bytes: [16]byte{2}, Valid: true}
	tests := []struct {
		name    string
		limit   *int64
		q       *fakeAdmitQueries
		wantErr error
	}{
		{
			name:  "nil limit is a no-op that never touches the database",
			limit: nil,
			q:     &fakeAdmitQueries{},
		},
		{
			name:    "under the limit admits",
			limit:   ptrOf(int64(3)),
			q:       &fakeAdmitQueries{snapshot: db.ChannelInstallationCapacitySnapshotRow{InstalledCount: 2, SameSlot: false}},
		},
		{
			name:  "at the limit with the same slot admits a reconnect",
			limit: ptrOf(int64(3)),
			q:     &fakeAdmitQueries{snapshot: db.ChannelInstallationCapacitySnapshotRow{InstalledCount: 3, SameSlot: true}},
		},
		{
			name:    "at the limit with a new slot refuses",
			limit:   ptrOf(int64(3)),
			q:       &fakeAdmitQueries{snapshot: db.ChannelInstallationCapacitySnapshotRow{InstalledCount: 3, SameSlot: false}},
			wantErr: ErrLimitReached,
		},
		{
			name:    "missing workspace refuses rather than crashing",
			limit:   ptrOf(int64(3)),
			q:       &fakeAdmitQueries{workspaceMissing: true},
			wantErr: ErrLimitReached,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := AdmitInstall(context.Background(), tt.q, ws, "slack", agent, tt.limit)
			if !errors.Is(err, tt.wantErr) {
				t.Fatalf("error = %v, want %v", err, tt.wantErr)
			}
		})
	}
	t.Run("nil limit never locks the workspace", func(t *testing.T) {
		q := &fakeAdmitQueries{}
		if err := AdmitInstall(context.Background(), q, ws, "slack", agent, nil); err != nil {
			t.Fatalf("error = %v", err)
		}
		if q.locked {
			t.Fatal("nil limit must not take the workspace lock")
		}
	})
}
