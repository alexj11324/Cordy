package devseed

import (
	"github.com/orvilo-ai/orvilo/server/internal/issueroles"
	"testing"
)

func TestUnassignedFixtureStatusesPassWorkflowGate(t *testing.T) {
	for _, status := range []string{"in_progress", "in_review", "blocked", "todo", "done", "backlog"} {
		initial := initialFixtureStatus(status)
		if violation := issueroles.WorkflowGate("", initial, nil, nil); violation != nil {
			t.Errorf("unassigned %s maps to invalid %s: %s", status, initial, *violation)
		}
	}
}
func TestAssignedReviewFixtureHasDistinctReviewer(t *testing.T) {
	executor := &issueroles.ActorRef{Type: "agent", ID: fixtureID("agent/runtime/test")}
	reviewer := &issueroles.ActorRef{Type: "member", ID: fixtureID("user/xu")}
	for _, issue := range issueFixtures() {
		if violation := issueroles.WorkflowGate("todo", issue.status, executor, reviewer); violation != nil {
			t.Errorf("fixture %s cannot enter %s: %s", issue.key, issue.status, *violation)
		}
	}
	if violation := issueroles.WorkflowGate("todo", "in_review", executor, nil); violation == nil {
		t.Fatal("review fixture must not omit reviewer")
	}
}
