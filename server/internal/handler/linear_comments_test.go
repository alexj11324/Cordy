package handler

import (
	"context"
	"encoding/json"
	"errors"
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
	if parentID != "" {
		if _, ok := a.comments[parentID]; !ok {
			return linearapi.Comment{}, errors.New("parent comment does not exist")
		}
	}
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
	createdAt := time.Now().UTC().Add(-24 * time.Hour)
	remote := linearapi.Comment{ID: "remote-comment", Body: "Hello", CreatedAt: createdAt, UpdatedAt: createdAt.Add(time.Hour)}
	remote.Issue.ID = linkedRemoteID(t, issueID)
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	if err = f.worker.applyLinearComment(ctx, b, remote, false); err != nil {
		t.Fatal(err)
	}
	var count, outbound int
	var persistedCreatedAt time.Time
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM comment WHERE issue_id=$1`, issueID).Scan(&count); err != nil {
		t.Fatal(err)
	}
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1 AND event_type LIKE 'comment_%'`, issueID).Scan(&outbound); err != nil {
		t.Fatal(err)
	}
	if err = testPool.QueryRow(ctx, `SELECT created_at FROM comment WHERE issue_id=$1`, issueID).Scan(&persistedCreatedAt); err != nil {
		t.Fatal(err)
	}
	if count != 1 || outbound != 0 {
		t.Fatalf("comments=%d outbound=%d", count, outbound)
	}
	if !persistedCreatedAt.Equal(createdAt) {
		t.Fatalf("created_at=%s, want provider timestamp %s", persistedCreatedAt, createdAt)
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

func TestLinearBindingSeedIncludesExistingComments(t *testing.T) {
	f := setupLinearWorker(t, "publish", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Existing discussion", testutil.Cols{"project_id": f.projectID})
	created, err := db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "member", AuthorID: parseUUID(testUserID), Content: "Before binding", Type: "comment"})
	if err != nil {
		t.Fatal(err)
	}
	reply, err := db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "member", AuthorID: parseUUID(testUserID), Content: "Existing reply", Type: "comment", ParentID: created.ID})
	if err != nil {
		t.Fatal(err)
	}
	systemParent, err := db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "system", AuthorID: parseUUID("00000000-0000-0000-0000-000000000000"), Content: "Internal progress", Type: "progress_update"})
	if err != nil {
		t.Fatal(err)
	}
	orphanedReply, err := db.New(testPool).CreateComment(ctx, db.CreateCommentParams{IssueID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID), AuthorType: "member", AuthorID: parseUUID(testUserID), Content: "Reply to internal progress", Type: "comment", ParentID: systemParent.ID})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = testPool.Exec(ctx, `DELETE FROM linear_sync_outbox WHERE binding_id=$1`, f.bindingID); err != nil {
		t.Fatal(err)
	}
	if _, err = testPool.Exec(ctx, `DELETE FROM linear_comment_link WHERE binding_id=$1`, f.bindingID); err != nil {
		t.Fatal(err)
	}
	tx, err := testPool.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err = testHandler.seedLinearOutbound(ctx, tx, parseUUID(testWorkspaceID), parseUUID(f.bindingID), parseUUID(f.projectID)); err != nil {
		_ = tx.Rollback(ctx)
		t.Fatal(err)
	}
	if err = tx.Commit(ctx); err != nil {
		t.Fatal(err)
	}
	var queued, linked int
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND event_type='comment_created' AND payload->>'comment_id'=$2`, f.bindingID, uuidToString(created.ID)).Scan(&queued); err != nil {
		t.Fatal(err)
	}
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_comment_link WHERE binding_id=$1 AND comment_id=$2`, f.bindingID, created.ID).Scan(&linked); err != nil {
		t.Fatal(err)
	}
	if queued != 1 || linked != 1 {
		t.Fatalf("queued=%d linked=%d", queued, linked)
	}
	var degraded int
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND payload->>'comment_id'=$2 AND payload->'parent_id'='null'::jsonb`, f.bindingID, uuidToString(orphanedReply.ID)).Scan(&degraded); err != nil {
		t.Fatal(err)
	}
	if degraded != 1 {
		t.Fatal("reply to excluded system comment was not seeded as a root")
	}
	api := &commentMemoryAPI{fakeLinearAPI: f.api, comments: map[string]linearapi.Comment{}}
	f.worker.api = api
	for range 4 {
		if !f.worker.processOneOutbox(ctx) {
			t.Fatal("seeded issue/comment outbox was not processed in FIFO order")
		}
	}
	if api.creates != 3 || len(api.comments) != 3 {
		t.Fatalf("seeded comments created=%d remote=%d", api.creates, len(api.comments))
	}
	var replyLinked int
	if err = testPool.QueryRow(ctx, `SELECT count(*) FROM linear_comment_link WHERE binding_id=$1 AND comment_id=$2`, f.bindingID, reply.ID).Scan(&replyLinked); err != nil {
		t.Fatal(err)
	}
	if replyLinked != 1 {
		t.Fatal("existing reply was not linked for publishing")
	}
}

func TestLinearCommentWebhookRetriesUntilIssueLinkExists(t *testing.T) {
	remoteIssue := linearapi.Issue{ID: "remote-pending-issue", ProjectID: "linear-project", TeamID: "linear-team", UpdatedAt: time.Now()}
	api := &fakeLinearAPI{listed: []linearapi.Issue{remoteIssue}}
	f := setupLinearWorker(t, "two_way", api)
	payload, err := json.Marshal(map[string]any{"action": "create", "type": "Comment", "data": map[string]any{"id": "remote-pending-comment", "issueId": remoteIssue.ID}})
	if err != nil {
		t.Fatal(err)
	}
	claim := linearClaim{ConnectionID: parseUUID(f.connectionID), Payload: payload}
	if err = f.worker.handleCommentInbox(context.Background(), claim); err == nil {
		t.Fatal("comment webhook was acknowledged before its issue link existed")
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
	if _, err := testPool.Exec(ctx, `UPDATE work_product SET external_url='https://github.com/acme/repo/pull/43',updated_at=now()+interval '1 second' WHERE id=$1`, productID); err != nil {
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
	for index, rawURL := range []string{"https://github.com/acme/repo/pull/42", "https://github.com/acme/repo/pull/43"} {
		if _, err := testPool.Exec(ctx, `UPDATE work_product SET external_url=$2,updated_at=now()+make_interval(secs => $3) WHERE id=$1`, productID, rawURL, index+2); err != nil {
			t.Fatal(err)
		}
		for range 2 {
			if !f.worker.processOneOutbox(ctx) {
				t.Fatal("repeated URL transition outbox not processed")
			}
		}
	}
	if len(api.attached) != 4 || api.attached[3] != linkedRemoteID(t, issueID)+"|PR #42|https://github.com/acme/repo/pull/43" {
		t.Fatalf("round-trip attachments=%v", api.attached)
	}
	if _, err := testPool.Exec(ctx, `UPDATE work_product_relation SET detached_at=now(),detached_by_type='user',detached_by_id=$2 WHERE id=$1`, relationID, testUserID); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("attachment deletion outbox not processed")
	}
	if len(api.detached) != 4 || api.detached[3] != linkedRemoteID(t, issueID)+"|https://github.com/acme/repo/pull/43" {
		t.Fatalf("deleted attachments=%v", api.detached)
	}
	product2 := dbfx.Insert(t, "work_product", testutil.Cols{"workspace_id": testWorkspaceID, "kind": "pull_request", "provider": "github", "external_identity": "PR #44", "external_url": "https://github.com/acme/repo/pull/44"})
	relation2 := dbfx.Insert(t, "work_product_relation", testutil.Cols{"workspace_id": testWorkspaceID, "work_product_id": product2, "issue_id": issueID, "relation_key": "test-pr-delete", "relation_source": "manual_explicit", "attached_by_type": "user", "attached_by_id": testUserID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("second attachment outbox not processed")
	}
	if _, err := testPool.Exec(ctx, `DELETE FROM work_product_relation WHERE id=$1`, relation2); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("hard-delete attachment outbox not processed")
	}
	if len(api.detached) != 5 || api.detached[4] != linkedRemoteID(t, issueID)+"|https://github.com/acme/repo/pull/44" {
		t.Fatalf("hard-deleted attachments=%v", api.detached)
	}
	httpProduct := dbfx.Insert(t, "work_product", testutil.Cols{"workspace_id": testWorkspaceID, "kind": "pull_request", "provider": "gitea", "external_identity": "PR #45", "external_url": "http://git.internal/acme/repo/pulls/45"})
	httpRelation := dbfx.Insert(t, "work_product_relation", testutil.Cols{"workspace_id": testWorkspaceID, "work_product_id": httpProduct, "issue_id": issueID, "relation_key": "test-http-pr", "relation_source": "manual_explicit", "attached_by_type": "user", "attached_by_id": testUserID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("HTTP PR relation prevented issue sync")
	}
	if len(api.attached) != 5 {
		t.Fatalf("unsupported HTTP attachment was published: %v", api.attached)
	}
	if _, err := testPool.Exec(ctx, `UPDATE work_product_relation SET detached_at=now(),detached_by_type='user',detached_by_id=$2 WHERE id=$1`, httpRelation, testUserID); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("HTTP PR detach was not acknowledged")
	}
	if len(api.detached) != 5 {
		t.Fatalf("unsupported HTTP attachment deletion reached provider: %v", api.detached)
	}
}

func TestLinearAttachmentDeletionWaitsForLastLiveRelation(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Shared PR relation", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue not published")
	}
	productID := dbfx.Insert(t, "work_product", testutil.Cols{"workspace_id": testWorkspaceID, "kind": "pull_request", "provider": "github", "external_identity": "PR shared", "external_url": "https://github.com/acme/repo/pull/50"})
	reference := dbfx.Insert(t, "work_product_relation", testutil.Cols{"workspace_id": testWorkspaceID, "work_product_id": productID, "issue_id": issueID, "relation_key": "provider-reference", "relation_source": "provider_reference", "attached_by_type": "system"})
	if _, err := testPool.Exec(ctx, `UPDATE work_product_relation SET detached_at=now(),detached_by_type='user',detached_by_id=$2 WHERE id=$1`, reference, testUserID); err != nil {
		t.Fatal(err)
	}
	var deletions int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND issue_id=$2 AND event_type='attachment_deleted'`, f.bindingID, issueID).Scan(&deletions); err != nil {
		t.Fatal(err)
	}
	if deletions != 0 {
		t.Fatalf("provider_reference detach queued %d deletion events", deletions)
	}
	relation1 := dbfx.Insert(t, "work_product_relation", testutil.Cols{"workspace_id": testWorkspaceID, "work_product_id": productID, "issue_id": issueID, "relation_key": "shared-1", "relation_source": "manual_explicit", "attached_by_type": "user", "attached_by_id": testUserID})
	relation2 := dbfx.Insert(t, "work_product_relation", testutil.Cols{"workspace_id": testWorkspaceID, "work_product_id": productID, "issue_id": issueID, "relation_key": "shared-2", "relation_source": "manual_explicit", "attached_by_type": "user", "attached_by_id": testUserID})
	if _, err := testPool.Exec(ctx, `UPDATE work_product_relation SET detached_at=now(),detached_by_type='user',detached_by_id=$2 WHERE id=$1`, relation1, testUserID); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND issue_id=$2 AND event_type='attachment_deleted'`, f.bindingID, issueID).Scan(&deletions); err != nil {
		t.Fatal(err)
	}
	if deletions != 0 {
		t.Fatalf("first detach queued %d attachment deletions while another relation remained", deletions)
	}
	if _, err := testPool.Exec(ctx, `UPDATE work_product_relation SET detached_at=now(),detached_by_type='user',detached_by_id=$2 WHERE id=$1`, relation2, testUserID); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND issue_id=$2 AND event_type='attachment_deleted'`, f.bindingID, issueID).Scan(&deletions); err != nil {
		t.Fatal(err)
	}
	if deletions != 1 {
		t.Fatalf("last detach queued %d attachment deletions, want 1", deletions)
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

func TestLinearReplyWaitsForUnseenParent(t *testing.T) {
	f := setupLinearWorker(t, "two_way", &fakeLinearAPI{})
	ctx := context.Background()
	issueID := dbfx.Issue(t, "Remote reply ordering", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(ctx) {
		t.Fatal("issue not published")
	}
	b, err := f.worker.loadBinding(ctx, parseUUID(f.bindingID))
	if err != nil {
		t.Fatal(err)
	}
	updatedAt := time.Now()
	parent := linearapi.Comment{ID: "remote-parent", Body: "Parent", UpdatedAt: updatedAt}
	parent.Issue.ID = linkedRemoteID(t, issueID)
	child := linearapi.Comment{ID: "remote-child", Body: "Child", UpdatedAt: updatedAt}
	child.Issue.ID = parent.Issue.ID
	if err = json.Unmarshal([]byte(`{"id":"remote-child","parent":{"id":"remote-parent"}}`), &child); err != nil {
		t.Fatal(err)
	}
	child.Body, child.UpdatedAt, child.Issue.ID = "Child", updatedAt, parent.Issue.ID
	if err = f.worker.applyLinearComment(ctx, b, child, false); err == nil {
		t.Fatal("child imported before its unseen parent")
	}
	if err = f.worker.applyLinearComment(ctx, b, parent, false); err != nil {
		t.Fatal(err)
	}
	if err = f.worker.applyLinearComment(ctx, b, child, false); err != nil {
		t.Fatal(err)
	}
	var linked bool
	if err = testPool.QueryRow(ctx, `SELECT child.parent_id=parent.id FROM comment child JOIN comment parent ON parent.id=child.parent_id JOIN linear_comment_link cl ON cl.comment_id=child.id WHERE cl.binding_id=$1 AND cl.linear_comment_id=$2`, f.bindingID, child.ID).Scan(&linked); err != nil {
		t.Fatal(err)
	}
	if !linked {
		t.Fatal("child was not linked to imported parent")
	}
}
