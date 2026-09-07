package main

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/analytics"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/realtime"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

// A channel or another transport may project a different payload from HTTP.
// Committed business side effects must still run from the creation facts.
func TestIssueCreationSideEffectsDoNotDependOnHTTPProjection(t *testing.T) {
	bus := events.New()
	app := newApplication(testPool, realtime.NewHub(), bus, analytics.NoopClient{}, nil, RouterOptions{})
	registerApplicationListeners(bus, testPool, app.queries)
	result, err := app.Services.Issues.Create(context.Background(), service.IssueCreateParams{
		WorkspaceID: util.MustParseUUID(testWorkspaceID),
		Title:       fmt.Sprintf("transport independent creation %d", time.Now().UnixNano()),
		Status:      "todo", Priority: "medium",
		CreatorType: "member", CreatorID: util.MustParseUUID(testUserID),
	}, service.IssueCreateOpts{
		BroadcastPayload: func(db.Issue, []db.Attachment, []db.IssueLabel) map[string]any {
			return map[string]any{"projection": "not an HTTP IssueResponse"}
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	issueID := util.UUIDToString(result.Issue.ID)
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM issue_subscriber WHERE issue_id = $1`, issueID)
		cleanupActivities(t, issueID)
		cleanupTestIssue(t, issueID)
	})
	if !isSubscribed(t, app.queries, issueID, "member", testUserID) {
		t.Fatal("creator subscription was lost when the transport projection changed")
	}
	activities := listActivitiesForIssue(t, app.queries, issueID)
	if len(activities) != 1 || activities[0].Action != "created" {
		t.Fatalf("committed creation activity = %#v", activities)
	}
}
