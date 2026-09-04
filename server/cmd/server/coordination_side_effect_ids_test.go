package main

import (
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/events"
)

func TestDurableCoordinationIDIsStableAndScoped(t *testing.T) {
	event := events.Event{
		Payload: map[string]any{
			"coordination_event_id": "11111111-1111-4111-8111-111111111111",
			"coordination_publication": "review_handoff",
		},
	}
	first, ok := durableCoordinationID(event, "inbox", "member", "recipient", "status_changed", "issue")
	if !ok || !first.Valid {
		t.Fatal("coordination publication should receive a durable id")
	}
	second, ok := durableCoordinationID(event, "inbox", "member", "recipient", "status_changed", "issue")
	if !ok || first != second {
		t.Fatalf("durable id changed on replay: first=%v second=%v", first, second)
	}
	otherRecipient, ok := durableCoordinationID(event, "inbox", "member", "other", "status_changed", "issue")
	if !ok || first == otherRecipient {
		t.Fatal("different recipients must not share a durable inbox id")
	}
	if _, ok := durableCoordinationID(events.Event{Payload: map[string]any{}}, "inbox"); ok {
		t.Fatal("ordinary events must retain random-id behavior")
	}
}

func TestExtractIssueForSideEffectReadsCoordinatorMap(t *testing.T) {
	reviewerType := "agent"
	reviewerID := "22222222-2222-4222-8222-222222222222"
	issue, ok := extractIssueForSideEffect(map[string]any{
		"coordination_event_id": "11111111-1111-4111-8111-111111111111",
		"issue": map[string]any{
			"id":            "33333333-3333-4333-8333-333333333333",
			"workspace_id":  "44444444-4444-4444-8444-444444444444",
			"title":         "review",
			"status":        "in_review",
			"priority":      "high",
			"creator_type":  "member",
			"creator_id":    "55555555-5555-4555-8555-555555555555",
			"reviewer_type": &reviewerType,
			"reviewer_id":   &reviewerID,
		},
	})
	if !ok {
		t.Fatal("coordinator map should be accepted by side-effect listeners")
	}
	if issue.Title != "review" || issue.Status != "in_review" || issue.Priority != "high" || issue.ReviewerID == nil || *issue.ReviewerID != reviewerID {
		t.Fatalf("extracted issue = %#v", issue)
	}
	if _, ok := extractIssueForSideEffect(map[string]any{
		"issue": map[string]any{
			"id": "33333333-3333-4333-8333-333333333333",
		},
	}); ok {
		t.Fatal("unmarked background map should not opt into side effects")
	}
}
