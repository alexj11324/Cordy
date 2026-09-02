package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sort"
	"testing"
)

func TestDeleteIssueDeletesDirectCommentAndTaskAttachments(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	originalStorage := testHandler.Storage
	store := &attachmentCleanupStorage{}
	testHandler.Storage = store
	t.Cleanup(func() { testHandler.Storage = originalStorage })

	issueID := createTestIssue(t, "issue attachment cleanup", "todo", "medium")
	agentID := createHandlerTestAgent(t, "IssueAttachmentCleanupAgent", []byte("[]"))

	var commentID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO comment (workspace_id, issue_id, author_type, author_id, content)
		VALUES ($1, $2, 'member', $3, 'issue attachment cleanup comment')
		RETURNING id
	`, testWorkspaceID, issueID, testUserID).Scan(&commentID); err != nil {
		t.Fatalf("create comment: %v", err)
	}

	var taskID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority)
		VALUES ($1, $2, $3, 'queued', 0)
		RETURNING id
	`, agentID, handlerTestRuntimeID(t), issueID).Scan(&taskID); err != nil {
		t.Fatalf("create task: %v", err)
	}

	attachmentIDs := make([]string, 0, 3)
	fixtures := []struct {
		filename  string
		url       string
		issueID   any
		commentID any
		taskID    any
	}{
		{filename: "direct.txt", url: "https://cdn.example.com/direct-owned", issueID: issueID},
		{filename: "comment.txt", url: "https://cdn.example.com/comment-owned", commentID: commentID},
		{filename: "task.txt", url: "https://cdn.example.com/task-owned", taskID: taskID},
	}
	for _, fixture := range fixtures {
		var attachmentID string
		if err := testPool.QueryRow(ctx, `
			INSERT INTO attachment (
				workspace_id, issue_id, comment_id, task_id, uploader_type,
				uploader_id, filename, url, content_type, size_bytes
			)
			VALUES ($1, $2, $3, $4, 'member', $5, $6, $7, 'text/plain', 1)
			RETURNING id
		`, testWorkspaceID, fixture.issueID, fixture.commentID, fixture.taskID,
			testUserID, fixture.filename, fixture.url).Scan(&attachmentID); err != nil {
			t.Fatalf("create %s attachment: %v", fixture.filename, err)
		}
		attachmentIDs = append(attachmentIDs, attachmentID)
	}

	req := newRequest(http.MethodDelete, "/api/issues/"+issueID, nil)
	req = withURLParam(req, "id", issueID)
	w := httptest.NewRecorder()
	testHandler.DeleteIssue(w, req)
	if w.Code != http.StatusNoContent {
		t.Fatalf("DeleteIssue: expected 204, got %d: %s", w.Code, w.Body.String())
	}

	var remaining int
	if err := testPool.QueryRow(ctx, `
		SELECT count(*)
		FROM attachment
		WHERE id = ANY($1::uuid[])
	`, attachmentIDs).Scan(&remaining); err != nil {
		t.Fatalf("count remaining attachments: %v", err)
	}
	if remaining != 0 {
		t.Fatalf("remaining issue-owned attachments = %d, want 0", remaining)
	}

	got := store.keys()
	sort.Strings(got)
	want := []string{"comment-owned", "direct-owned", "task-owned"}
	if len(got) != len(want) {
		t.Fatalf("deleted storage keys = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("deleted storage keys = %v, want %v", got, want)
		}
	}
}
