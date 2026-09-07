package linearsync

import (
	"context"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestLinearWorkerBroadcastsRemoteProjectMoveWithoutEcho(t *testing.T) {
	ctx := context.Background()
	f := setupWorker(t, "two_way", &fakeLinearAPI{})
	issueID := dbfx.Issue(t, "Shared title", testutil.Cols{"project_id": f.projectID, "description": "Shared body"})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue was not published")
	}
	targetProjectID := dbfx.Project(t, "Move destination")
	targetBindingID := dbfx.Insert(t, "linear_project_binding", testutil.Cols{
		"workspace_id": testWorkspaceID, "connection_id": f.connectionID,
		"orvilo_project_id": targetProjectID, "linear_project_id": "linear-destination", "linear_team_id": "linear-team",
		"status": "active", "sync_mode": "two_way", "initial_source_of_truth": "linear",
		"status_mapping": testutil.Raw(`'{"remote-todo":"todo"}'::jsonb`), "agent_label_mapping": testutil.Raw("'{}'::jsonb"),
		"created_by_id": testUserID,
	})
	sink := &captureSyncEvents{}
	f.worker.events = sink
	before := issueRevision(t, issueID)
	at := time.Now().Add(time.Second)
	payload := webhookPayload(t, "update", at.UnixMilli(), map[string]any{
		"id": linkedRemoteID(t, issueID), "identifier": "ENG-1", "title": "Shared title", "description": "Shared body", "priority": 0,
		"updatedAt": at.UTC().Format(time.RFC3339Nano), "project": map[string]any{"id": "linear-destination"},
		"team": map[string]any{"id": "linear-team"}, "state": map[string]any{"id": "remote-todo", "type": "unstarted"},
	})
	f.queueWebhook(t, "remote-project-move", payload)
	if !f.worker.processOneInbox(ctx) {
		t.Fatal("move was not processed")
	}
	var projectID, bindingID string
	dbfx.QueryRow(t, `SELECT i.project_id,l.binding_id FROM issue i JOIN linear_issue_link l ON l.orvilo_issue_id=i.id WHERE i.id=$1`, issueID).Scan(&projectID, &bindingID)
	if projectID != targetProjectID || bindingID != targetBindingID {
		t.Fatalf("move did not commit: project=%q binding=%q", projectID, bindingID)
	}
	if len(sink.issues) != 1 {
		t.Fatalf("pure project move emitted %d events, want 1", len(sink.issues))
	}
	if !sink.projectChanges[0] {
		t.Fatal("move event omitted the authoritative project change")
	}
	if got := sink.issues[0]; uuidToString(got.ID) != issueID || uuidToString(got.ProjectID) != targetProjectID || got.Revision != before+1 {
		t.Fatalf("move event lost updated issue: %+v", got)
	}
	if sink.actorTypes[0] != "system" || sink.actorIDs[0] != "00000000-0000-0000-0000-000000000000" {
		t.Fatalf("move actor = %q/%q", sink.actorTypes[0], sink.actorIDs[0])
	}
	f.queueWebhook(t, "remote-project-move-replay", payload)
	if !f.worker.processOneInbox(ctx) {
		t.Fatal("move replay was not processed")
	}
	if after := issueRevision(t, issueID); after != before+1 || len(sink.issues) != 1 {
		t.Fatalf("move replay mutated again: revision=%d events=%d", after, len(sink.issues))
	}
	if pending := dbfx.Count(t, `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1 AND processed_at IS NULL`, issueID); pending != 0 {
		t.Fatalf("remote move queued %d outbound echoes", pending)
	}
	if created, updated, deleted, _ := f.api.calls(); created != 1 || updated != 0 || deleted != 0 {
		t.Fatalf("move echoed to provider: created=%d updated=%d deleted=%d", created, updated, deleted)
	}
}
