package handler

import (
	"context"
	"errors"
	"slices"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/issuestatus"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestProjectTerminalIssueStatusKeysFallsBackToCanonicalKeys(t *testing.T) {
	h := *testHandler
	h.Queries = db.New(failQueryDBTX{
		DBTX:   testPool,
		failOn: "SELECT key FROM issue_status",
		err:    errors.New("status catalog unavailable"),
	})

	got := h.projectTerminalIssueStatusKeys(context.Background(), parseUUID(testWorkspaceID))
	want := []string{issuestatus.Done, issuestatus.Cancelled}
	if !slices.Equal(got, want) {
		t.Fatalf("project terminal keys = %v, want canonical fallback %v", got, want)
	}
}
