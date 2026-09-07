package handler

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/events"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func TestLinearEventSinkPreservesIssueEventEnvelope(t *testing.T) {
	bus := events.New()
	var got events.Event
	bus.Subscribe(protocol.EventIssueUpdated, func(event events.Event) { got = event })

	issue := db.Issue{
		ID:          parseUUID("10000000-0000-0000-0000-000000000001"),
		WorkspaceID: parseUUID("20000000-0000-0000-0000-000000000002"),
		CreatorID:   parseUUID("30000000-0000-0000-0000-000000000003"),
		CreatorType: "system",
		Number:      42,
		Title:       "Imported from Linear",
		Status:      "todo",
		Revision:    7,
	}
	NewLinearEventSink(bus).IssueChanged(issue, protocol.EventIssueUpdated, "member", "40000000-0000-0000-0000-000000000004", false)

	if got.Type != protocol.EventIssueUpdated || got.WorkspaceID != uuidToString(issue.WorkspaceID) {
		t.Fatalf("event routing = (%q, %q), want (%q, %q)", got.Type, got.WorkspaceID, protocol.EventIssueUpdated, uuidToString(issue.WorkspaceID))
	}
	if got.ActorType != "member" || got.ActorID != "40000000-0000-0000-0000-000000000004" {
		t.Fatalf("event actor = (%q, %q)", got.ActorType, got.ActorID)
	}
	if got.TaskID != uuidToString(issue.ID) {
		t.Fatalf("task id = %q, want %q", got.TaskID, uuidToString(issue.ID))
	}
	payload, ok := got.Payload.(map[string]any)
	if !ok {
		t.Fatalf("payload type = %T", got.Payload)
	}
	rendered, ok := payload["issue"].(map[string]any)
	if !ok {
		t.Fatalf("issue payload type = %T", payload["issue"])
	}
	if rendered["id"] != uuidToString(issue.ID) || rendered["title"] != issue.Title || rendered["revision"] != issue.Revision {
		t.Fatalf("issue payload lost canonical fields: %#v", rendered)
	}
	if payload["project_changed"] != false {
		t.Fatalf("unchanged project flag = %#v", payload["project_changed"])
	}
	issue.ProjectID = parseUUID("50000000-0000-0000-0000-000000000005")
	issue.Revision++
	NewLinearEventSink(bus).IssueChanged(issue, protocol.EventIssueUpdated, "system", "00000000-0000-0000-0000-000000000000", true)
	payload = got.Payload.(map[string]any)
	rendered = payload["issue"].(map[string]any)
	projectID, ok := rendered["project_id"].(*string)
	if payload["project_changed"] != true || !ok || projectID == nil || *projectID != uuidToString(issue.ProjectID) || rendered["revision"] != issue.Revision {
		t.Fatalf("project move lost cache reconciliation fields: %#v", payload)
	}
}

func TestLinearEventSinkPreservesCommentEventPayload(t *testing.T) {
	for _, tc := range []struct {
		name      string
		created   bool
		eventType string
	}{
		{name: "created", created: true, eventType: protocol.EventCommentCreated},
		{name: "updated", created: false, eventType: protocol.EventCommentUpdated},
	} {
		t.Run(tc.name, func(t *testing.T) {
			bus := events.New()
			var got events.Event
			bus.Subscribe(tc.eventType, func(event events.Event) { got = event })
			comment := db.Comment{
				ID:          parseUUID("50000000-0000-0000-0000-000000000005"),
				IssueID:     parseUUID("60000000-0000-0000-0000-000000000006"),
				WorkspaceID: parseUUID("70000000-0000-0000-0000-000000000007"),
				AuthorType:  "system",
				AuthorID:    parseUUID("00000000-0000-0000-0000-000000000000"),
				Content:     "Linear comment",
				Type:        "comment",
				Revision:    3,
			}
			NewLinearEventSink(bus).CommentChanged(comment, 11, tc.created)

			if got.Type != tc.eventType || got.WorkspaceID != uuidToString(comment.WorkspaceID) || got.TaskID != uuidToString(comment.IssueID) {
				t.Fatalf("event routing = type %q workspace %q task %q", got.Type, got.WorkspaceID, got.TaskID)
			}
			if got.ActorType != "system" || got.ActorID != "00000000-0000-0000-0000-000000000000" {
				t.Fatalf("event actor = (%q, %q)", got.ActorType, got.ActorID)
			}
			payload, ok := got.Payload.(map[string]any)
			if !ok {
				t.Fatalf("payload type = %T", got.Payload)
			}
			response, ok := payload["comment"].(CommentResponse)
			if !ok {
				t.Fatalf("comment payload type = %T", payload["comment"])
			}
			if response.ID != uuidToString(comment.ID) || response.Content != comment.Content || response.IssueRevision != 11 {
				t.Fatalf("comment response lost canonical fields: %#v", response)
			}
			if payload["issue_revision"] != int64(11) {
				t.Fatalf("issue_revision = %#v", payload["issue_revision"])
			}
		})
	}
}

// linearTestSignature reproduces the HMAC the webhook endpoint verifies.
func linearTestSignature(t *testing.T, secret string, body []byte) string {
	t.Helper()
	mac := hmac.New(sha256.New, []byte(secret))
	if _, err := mac.Write(body); err != nil {
		t.Fatal(err)
	}
	return hex.EncodeToString(mac.Sum(nil))
}
