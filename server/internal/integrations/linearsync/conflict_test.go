package linearsync

import (
	"context"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type captureSyncEvents struct {
	issues     []db.Issue
	actorTypes []string
	actorIDs   []string
}

func (s *captureSyncEvents) IssueChanged(issue db.Issue, _ string, actorType, actorID string) {
	s.issues = append(s.issues, issue)
	s.actorTypes = append(s.actorTypes, actorType)
	s.actorIDs = append(s.actorIDs, actorID)
}
func (*captureSyncEvents) CommentChanged(db.Comment, int64, bool) {}

func TestLinearConflictUseCaseCommitsResolutionBeforePublishing(t *testing.T) {
	f := setupWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Shared title", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue did not publish")
	}
	if _, err := testPool.Exec(ctx, `UPDATE issue SET title='Local title',revision=revision+1 WHERE id=$1`, issueID); err != nil {
		t.Fatal(err)
	}
	binding, err := f.worker.loadBinding(ctx, parseUUID(f.bindingID))
	if err != nil {
		t.Fatal(err)
	}
	remote := f.api.listed[0]
	remote.Title = "Remote title"
	remote.UpdatedAt = remote.UpdatedAt.Add(time.Second)
	if err = f.worker.applyRemote(ctx, binding, remote, "conflict-resolution-test", remote.UpdatedAt.UnixMilli()); err != nil {
		t.Fatal(err)
	}
	var conflictID pgtype.UUID
	if err = testPool.QueryRow(ctx, `SELECT id FROM linear_sync_conflict WHERE orvilo_issue_id=$1 AND field='title' AND status='open'`, issueID).Scan(&conflictID); err != nil {
		t.Fatal(err)
	}
	sink := &captureSyncEvents{}
	f.worker.events = sink
	conflict, issue, err := f.worker.ResolveConflict(ctx, parseUUID(testWorkspaceID), conflictID, parseUUID(testUserID), "remote", nil)
	if err != nil {
		t.Fatal(err)
	}
	if issue.Title != "Remote title" || conflict.Status != "resolved" {
		t.Fatalf("resolution returned issue=%q conflict=%q", issue.Title, conflict.Status)
	}
	var title, status string
	if err = testPool.QueryRow(ctx, `SELECT i.title,l.sync_status FROM issue i JOIN linear_issue_link l ON l.orvilo_issue_id=i.id WHERE i.id=$1`, issueID).Scan(&title, &status); err != nil {
		t.Fatal(err)
	}
	if title != "Remote title" || status != "active" {
		t.Fatalf("committed title=%q link=%q", title, status)
	}
	if len(sink.issues) != 1 || sink.issues[0].Title != title || sink.actorTypes[0] != "member" || sink.actorIDs[0] != testUserID {
		t.Fatalf("resolution events=%+v", sink)
	}
	if _, _, err = f.worker.ResolveConflict(ctx, parseUUID(testWorkspaceID), conflictID, parseUUID(testUserID), "remote", nil); err == nil {
		t.Fatal("resolved conflict was applied twice")
	}
	if len(sink.issues) != 1 {
		t.Fatal("rejected repeated resolution published another change")
	}
}
