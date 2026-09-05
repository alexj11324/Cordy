package handler

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	linearapi "github.com/patchbay-ai/patchbay/server/internal/integrations/linear"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type commentMemoryAPI struct {
	*fakeLinearAPI
	comments map[string]linearapi.Comment
	creates  int
}

func (a *commentMemoryAPI) FetchComment(_ context.Context, _ string, id string) (linearapi.Comment, bool, error) {
	c, ok := a.comments[id]
	return c, ok, nil
}
func (a *commentMemoryAPI) CreateComment(_ context.Context, _ string, id, issueID, parentID, body, author string) (linearapi.Comment, error) {
	a.creates++
	c := linearapi.Comment{ID: id, Body: body, UpdatedAt: time.Now()}
	c.Issue.ID = issueID
	a.comments[id] = c
	return c, nil
}
func (a *commentMemoryAPI) UpdateComment(_ context.Context, _ string, id, body string) error {
	c := a.comments[id]
	c.Body = body
	a.comments[id] = c
	return nil
}
func (a *commentMemoryAPI) DeleteComment(_ context.Context, _ string, id string) error {
	delete(a.comments, id)
	return nil
}

func TestLinearCommentsOutboundRetryUsesOneRemoteComment(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Outbound comments", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue not published")
	}
	created, err := db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "member", AuthorID: parseUUID(testUserID), Content: "Hello", Type: "comment"})
	if err != nil {
		t.Fatal(err)
	}
	api := &commentMemoryAPI{fakeLinearAPI: f.api, comments: map[string]linearapi.Comment{}}
	f.worker.api = api
	b, err := f.worker.loadBinding(ctx, parseUUID(f.bindingID))
	if err != nil {
		t.Fatal(err)
	}
	payload, _ := json.Marshal(map[string]any{"comment_id": uuidToString(created.ID), "body": "Hello", "author_id": testUserID, "author_type": "member"})
	c := linearOutboxClaim{WorkspaceID: parseUUID(testWorkspaceID), BindingID: b.ID, IssueID: parseUUID(issueID), EventType: "comment_created", Payload: payload}
	for i := 0; i < 2; i++ {
		if err = f.worker.handleCommentOutbox(ctx, c, b, "fixture"); err != nil {
			t.Fatal(err)
		}
	}
	if api.creates != 1 || len(api.comments) != 1 {
		t.Fatalf("creates=%d comments=%d", api.creates, len(api.comments))
	}
	c.EventType = "comment_deleted"
	if err = f.worker.handleCommentOutbox(ctx, c, b, "fixture"); err != nil {
		t.Fatal(err)
	}
	if len(api.comments) != 0 {
		t.Fatal("comment deletion was not sent")
	}
}

func TestLinearCommentsImportDeduplicatesAndDoesNotEcho(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Comments", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue not published")
	}
	b, err := f.worker.loadBinding(ctx, parseUUID(f.bindingID))
	if err != nil {
		t.Fatal(err)
	}
	remote := linearapi.Comment{ID: "remote-comment", Body: "Hello", UpdatedAt: time.Now().UTC()}
	remote.Issue.ID = linkedRemoteID(t, issueID)
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	var count, outbound int
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM comment WHERE issue_id=$1`, issueID).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1 AND event_type LIKE 'comment_%'`, issueID).Scan(&outbound); err != nil {
		t.Fatal(err)
	}
	if count != 1 || outbound != 0 {
		t.Fatalf("comments=%d outbound=%d", count, outbound)
	}
	newer := remote
	newer.Body = "Edited"
	newer.UpdatedAt = remote.UpdatedAt.Add(time.Second)
	if err = f.worker.applyLinearComment(ctx, b, newer, false); err != nil {
		t.Fatal(err)
	}
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	var body string
	if err = testPool.QueryRow(ctx, `SELECT content FROM comment WHERE issue_id=$1`, issueID).Scan(&body); err != nil {
		t.Fatal(err)
	}
	if body != "Linear user · Linear\n\nEdited" {
		t.Fatalf("stale event overwrote comment: %s", body)
	}
}

func TestLinearCommentsLocalWriteQueuesDurably(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Local discussion", testutil.Cols{"project_id": f.projectID})
	created, err := db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "member", AuthorID: parseUUID(testUserID), Content: "Reply", Type: "comment"})
	if err != nil {
		t.Fatal(err)
	}
	var queued int
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND event_type='comment_created' AND payload->>'comment_id'=$2`, f.bindingID, uuidToString(created.ID)).Scan(&queued); err != nil {
		t.Fatal(err)
	}
	if queued != 1 {
		t.Fatalf("queued=%d", queued)
	}
	if _, err = db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "agent", AuthorID: parseUUID(testUserID), Content: "internal progress", Type: "progress_update"}); err != nil {
		t.Fatal(err)
	}
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND event_type='comment_created'`, f.bindingID).Scan(&queued); err != nil {
		t.Fatal(err)
	}
	if queued != 1 {
		t.Fatalf("internal progress was exported: queued=%d", queued)
	}
	var remoteID string
	if err = testPool.QueryRow(ctx, `SELECT linear_comment_id FROM linear_comment_link WHERE binding_id=$1 AND comment_id=$2`, f.bindingID, created.ID).Scan(&remoteID); err != nil {
		t.Fatal(err)
	}
	if len(remoteID) != 36 {
		t.Fatalf("missing durable provider ID: %q", remoteID)
	}
}

func TestLinearWorkProductPublishesPullRequestAttachment(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	ctx := context.Background()
	issueID := dbfx.Issue(t, "PR attachment", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue not published")
	}
	productID := dbfx.Insert(t, "work_product", testutil.Cols{"workspace_id": testWorkspaceID, "kind": "pull_request", "provider": "github", "external_identity": "PR #42", "external_url": "https://github.com/acme/repo/pull/42"})
	relationID := dbfx.Insert(t, "work_product_relation", testutil.Cols{"workspace_id": testWorkspaceID, "work_product_id": productID, "issue_id": issueID, "relation_key": "test-pr", "relation_source": "manual_explicit", "attached_by_type": "user", "attached_by_id": testUserID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("attachment outbox not processed")
	}
	if len(api.attached) != 1 || api.attached[0] != linkedRemoteID(t, issueID)+"|PR #42|https://github.com/acme/repo/pull/42" {
		t.Fatalf("attachments=%v", api.attached)
	}
	if _, err := testPool.Exec(ctx, `UPDATE work_product SET external_url='https://github.com/acme/repo/pull/43' WHERE id=$1`, productID); err != nil {
		t.Fatal(err)
	}
	for range 2 {
		if !f.worker.processOneOutbox(ctx) {
			t.Fatal("URL replacement outbox not processed")
		}
	}
	if len(api.attached) != 2 || api.attached[1] != linkedRemoteID(t, issueID)+"|PR #42|https://github.com/acme/repo/pull/43" {
		t.Fatalf("replacement attachments=%v", api.attached)
	}
	if len(api.detached) != 1 || api.detached[0] != linkedRemoteID(t, issueID)+"|https://github.com/acme/repo/pull/42" {
		t.Fatalf("replaced attachments not deleted: %v", api.detached)
	}
	if _, err := testPool.Exec(ctx, `UPDATE work_product_relation SET detached_at=now(),detached_by_type='user',detached_by_id=$2 WHERE id=$1`, relationID, testUserID); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("attachment deletion outbox not processed")
	}
	if len(api.detached) != 2 || api.detached[1] != linkedRemoteID(t, issueID)+"|https://github.com/acme/repo/pull/43" {
		t.Fatalf("deleted attachments=%v", api.detached)
	}
}

func TestLinearImportedCommentReappearsAfterLocalDeletion(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Remote discussion recovery", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue not published")
	}
	remoteIssueID := linkedRemoteID(t, issueID)
	b, err := f.worker.loadBinding(ctx, parseUUID(f.bindingID))
	if err != nil {
		t.Fatal(err)
	}
	remote := linearapi.Comment{ID: "remote-comment-recovery", Body: "Original", UpdatedAt: time.Now().Add(-time.Minute)}
	remote.Issue.ID = remoteIssueID
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	var localID string
	if err = testPool.QueryRow(ctx, `SELECT comment_id FROM linear_comment_link WHERE binding_id=$1 AND linear_comment_id=$2`, f.bindingID, remote.ID).Scan(&localID); err != nil {
		t.Fatal(err)
	}
	if _, err = testPool.Exec(ctx, `DELETE FROM comment WHERE id=$1`, localID); err != nil {
		t.Fatal(err)
	}
	remote.Body = "Restored"
	remote.UpdatedAt = time.Now()
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	var body string
	if err = testPool.QueryRow(ctx, `SELECT content FROM comment WHERE id=$1`, localID).Scan(&body); err != nil {
		t.Fatal(err)
	}
	if body != "Linear user · Linear\n\nRestored" {
		t.Fatalf("restored body=%q", body)
	}
}
