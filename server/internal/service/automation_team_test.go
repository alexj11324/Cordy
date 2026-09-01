package service

import (
	"errors"
	"fmt"
	"testing"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestAutomationTeamAttribution(t *testing.T) {
	id := pgtype.UUID{Valid: true}
	copy(id.Bytes[:], []byte("01234567890123456789012345678901"))

	tests := []struct {
		name string
		ap   db.Automation
		want pgtype.UUID
	}{
		{"agent assignee returns zero", db.Automation{AssigneeType: "agent", AssigneeID: id}, pgtype.UUID{}},
		{"team assignee returns team id", db.Automation{AssigneeType: "team", AssigneeID: id}, id},
		{"team with invalid id returns zero", db.Automation{AssigneeType: "team", AssigneeID: pgtype.UUID{}}, pgtype.UUID{}},
		{"unset type defaults to non-team", db.Automation{AssigneeID: id}, pgtype.UUID{}},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := automationTeamAttribution(tc.ap)
			if got.Valid != tc.want.Valid {
				t.Fatalf("Valid mismatch: got %v want %v", got.Valid, tc.want.Valid)
			}
			if got.Valid && got.Bytes != tc.want.Bytes {
				t.Fatalf("Bytes mismatch")
			}
		})
	}
}

func TestFormatAdmissionReason(t *testing.T) {
	tests := []struct {
		name string
		ap   db.Automation
		raw  string
		want string
	}{
		{"agent archived", db.Automation{AssigneeType: "agent"}, "agent is archived", "assignee agent is archived"},
		{"team archived", db.Automation{AssigneeType: "team"}, "agent is archived", "team leader agent is archived"},
		{"agent no runtime", db.Automation{AssigneeType: "agent"}, "agent has no runtime bound", "assignee agent has no runtime bound"},
		{"team no runtime", db.Automation{AssigneeType: "team"}, "agent has no runtime bound", "team leader agent has no runtime bound"},
		{"runtime offline retains MUL-1899 suffix", db.Automation{AssigneeType: "agent"}, "agent runtime is offline", "agent runtime is offline at dispatch time"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if got := formatAdmissionReason(tc.ap, tc.raw); got != tc.want {
				t.Fatalf("got %q want %q", got, tc.want)
			}
		})
	}
}

// errDispatchSkipped must be distinguishable via errors.As from a wrapped
// fmt.Errorf, otherwise DispatchAutomation's failure-vs-skip switch will treat
// it as a generic failure and the manual-trigger handler will 500. Locks in
// the contract that fixed the post-admission race (PR #2888 review fix #2).
func TestErrDispatchSkippedUnwraps(t *testing.T) {
	base := &errDispatchSkipped{reason: "team leader agent is archived"}
	wrapped := fmt.Errorf("dispatch run_only: %w", base)

	var got *errDispatchSkipped
	if !errors.As(wrapped, &got) {
		t.Fatalf("errors.As did not match errDispatchSkipped through fmt.Errorf wrap")
	}
	if got.reason != base.reason {
		t.Fatalf("reason mismatch: got %q want %q", got.reason, base.reason)
	}

	// pgx.ErrNoRows must NOT pass through the same gate — otherwise transient
	// "row not found" errors that should fail-open via shouldSkipDispatch
	// would be swallowed silently as skips at the dispatch level.
	if errors.As(pgx.ErrNoRows, &got) {
		t.Fatal("pgx.ErrNoRows wrongly satisfied errDispatchSkipped")
	}
}

func TestResolveAutomationLeaderSentinels(t *testing.T) {
	// Sanity-check the sentinel exported via errors.Is so callers can branch
	// on "archived" without string-matching the failure reason.
	if !errors.Is(errTeamArchived, errTeamArchived) {
		t.Fatal("errTeamArchived must satisfy errors.Is itself")
	}
	wrapped := fmt.Errorf("wrap: %w", errTeamArchived)
	if !errors.Is(wrapped, errTeamArchived) {
		t.Fatal("errTeamArchived must unwrap through fmt.Errorf")
	}
}
