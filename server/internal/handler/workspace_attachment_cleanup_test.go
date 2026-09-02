package handler

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"sort"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

type workspaceAttachmentCleanupFixture struct {
	workspaceID    string
	neighborID     string
	attachmentKeys []string
	neighborKey    string
}

// newWorkspaceAttachmentCleanupFixture creates one row for every current
// attachment ownership shape. The workspace-only row represents an unattached
// upload; the task-bound row intentionally uses task_id without a foreign key,
// matching the transient ownership contract in migration 164.
func newWorkspaceAttachmentCleanupFixture(t *testing.T) workspaceAttachmentCleanupFixture {
	t.Helper()
	ctx := context.Background()
	suffix := fmt.Sprintf("%d", time.Now().UnixNano())

	insertWorkspace := func(name, slug string) string {
		var id string
		if err := testPool.QueryRow(ctx, `
INSERT INTO workspace (name, slug)
VALUES ($1, $2)
RETURNING id
`, name, slug).Scan(&id); err != nil {
			t.Fatalf("create workspace %s: %v", slug, err)
		}
		return id
	}

	targetID := insertWorkspace("Workspace attachment cleanup target", "handler-tests-attachment-target-"+suffix)
	neighborID := insertWorkspace("Workspace attachment cleanup neighbor", "handler-tests-attachment-neighbor-"+suffix)
	if _, err := testPool.Exec(ctx, `
INSERT INTO member (workspace_id, user_id, role)
VALUES ($1, $2, 'owner')
`, targetID, testUserID); err != nil {
		t.Fatalf("create workspace owner: %v", err)
	}

	// The cleanup is deliberately explicit for source-context rows: that table
	// predates a workspace FK and must not be left behind by test teardown.
	cleanupIDs := []string{targetID, neighborID}
	t.Cleanup(func() {
		cleanupCtx := context.Background()
		_, _ = testPool.Exec(cleanupCtx, `
DELETE FROM issue_source_context_object_intent
WHERE workspace_id = ANY($1::uuid[])
`, cleanupIDs)
		_, _ = testPool.Exec(cleanupCtx, `
DELETE FROM issue_source_context
WHERE workspace_id = ANY($1::uuid[])
`, cleanupIDs)
		_, _ = testPool.Exec(cleanupCtx, `
DELETE FROM workspace
WHERE id = ANY($1::uuid[])
`, cleanupIDs)
	})

	var issueID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO issue (workspace_id, title, creator_type, creator_id)
VALUES ($1, 'workspace attachment cleanup issue', 'member', $2)
RETURNING id
`, targetID, testUserID).Scan(&issueID); err != nil {
		t.Fatalf("create issue: %v", err)
	}
	var commentID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO comment (issue_id, workspace_id, author_type, author_id, content)
VALUES ($1, $2, 'member', $3, 'workspace attachment cleanup comment')
RETURNING id
`, issueID, targetID, testUserID).Scan(&commentID); err != nil {
		t.Fatalf("create comment: %v", err)
	}

	var runtimeID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO agent_runtime (
    workspace_id, name, runtime_mode, provider, status, device_info, metadata, owner_id
)
VALUES ($1, 'workspace attachment cleanup runtime', 'cloud', 'attachment-cleanup', 'offline', '', '{}'::jsonb, $2)
RETURNING id
`, targetID, testUserID).Scan(&runtimeID); err != nil {
		t.Fatalf("create runtime: %v", err)
	}
	var agentID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO agent (workspace_id, name, runtime_mode, runtime_config, runtime_id, owner_id)
VALUES ($1, 'workspace attachment cleanup agent', 'cloud', '{}'::jsonb, $2, $3)
RETURNING id
`, targetID, runtimeID, testUserID).Scan(&agentID); err != nil {
		t.Fatalf("create agent: %v", err)
	}
	var sessionID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO chat_session (workspace_id, agent_id, creator_id, title, status)
VALUES ($1, $2, $3, 'workspace attachment cleanup session', 'active')
RETURNING id
`, targetID, agentID, testUserID).Scan(&sessionID); err != nil {
		t.Fatalf("create chat session: %v", err)
	}
	var messageID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO chat_message (chat_session_id, role, content)
VALUES ($1, 'assistant', 'workspace attachment cleanup message')
RETURNING id
`, sessionID).Scan(&messageID); err != nil {
		t.Fatalf("create chat message: %v", err)
	}

	var sourceContextID string
	if err := testPool.QueryRow(ctx, `
INSERT INTO issue_source_context (
    id, workspace_id, issue_id, source_issue_id, anchor_comment_id,
    captured_by_user_id, snapshot_version, snapshot, capture_digest,
    state, attached_at
)
VALUES (
    gen_random_uuid(), $1, $2, $2, $3, $4, 1, '{}'::jsonb,
    'workspace-attachment-cleanup', 'attached', now()
)
RETURNING id
`, targetID, issueID, commentID, testUserID).Scan(&sourceContextID); err != nil {
		t.Fatalf("create source context: %v", err)
	}

	attachmentKeys := []string{
		"workspace-attachment-unattached-" + suffix,
		"workspace-attachment-issue-" + suffix,
		"workspace-attachment-comment-" + suffix,
		"workspace-attachment-chat-session-" + suffix,
		"workspace-attachment-chat-message-" + suffix,
		"workspace-attachment-task-" + suffix,
		"workspace-attachment-source-context-" + suffix,
	}
	nullUUID := func(value string) any {
		if value == "" {
			return nil
		}
		return value
	}
	insertAttachment := func(key, issueID, commentID, sessionID, messageID, sourceContextID string, taskBound bool) {
		t.Helper()
		var taskID any
		if taskBound {
			taskID = uuid.NewString()
		}
		if _, err := testPool.Exec(ctx, `
INSERT INTO attachment (
    workspace_id, issue_id, comment_id, chat_session_id, chat_message_id,
    task_id, source_context_id, uploader_type, uploader_id, filename,
    url, content_type, size_bytes
)
VALUES ($1, $2, $3, $4, $5, $6, $7, 'member', $8, $9, $10, 'text/plain', 1)
`, targetID, nullUUID(issueID), nullUUID(commentID), nullUUID(sessionID),
			nullUUID(messageID), taskID, nullUUID(sourceContextID), testUserID,
			key+".txt", "https://cdn.example.com/"+key); err != nil {
			t.Fatalf("create attachment %s: %v", key, err)
		}
	}
	insertAttachment(attachmentKeys[0], "", "", "", "", "", false)
	insertAttachment(attachmentKeys[1], issueID, "", "", "", "", false)
	insertAttachment(attachmentKeys[2], issueID, commentID, "", "", "", false)
	insertAttachment(attachmentKeys[3], "", "", sessionID, "", "", false)
	insertAttachment(attachmentKeys[4], "", "", sessionID, messageID, "", false)
	insertAttachment(attachmentKeys[5], "", "", "", "", "", true)
	insertAttachment(attachmentKeys[6], "", "", "", "", sourceContextID, false)

	intentKey := "workspace-attachment-intent-" + suffix
	if _, err := testPool.Exec(ctx, `
INSERT INTO issue_source_context_object_intent (
    storage_key, workspace_id, source_context_id, attachment_id, object_url
)
VALUES ($1, $2, $3, gen_random_uuid(), $4)
`, intentKey, targetID, sourceContextID, "s3://workspace-attachment/"+intentKey); err != nil {
		t.Fatalf("create source-context object intent: %v", err)
	}

	neighborKey := "workspace-attachment-neighbor-" + suffix
	if _, err := testPool.Exec(ctx, `
INSERT INTO attachment (
    workspace_id, uploader_type, uploader_id, filename, url, content_type, size_bytes
)
VALUES ($1, 'member', $2, 'neighbor.txt', $3, 'text/plain', 1)
`, neighborID, testUserID, "https://cdn.example.com/"+neighborKey); err != nil {
		t.Fatalf("create neighbor attachment: %v", err)
	}

	expectedKeys := append(append([]string(nil), attachmentKeys...), "s3://workspace-attachment/"+intentKey)
	return workspaceAttachmentCleanupFixture{
		workspaceID:    targetID,
		neighborID:     neighborID,
		attachmentKeys: expectedKeys,
		neighborKey:    neighborKey,
	}
}

type workspaceAttachmentStorageProbe struct {
	mockStorage
	pool        *pgxpool.Pool
	workspaceID string

	deletedMu           sync.Mutex
	deletedKeys         []string
	observationMu       sync.Mutex
	observedAfterCommit bool
	observationErr      error
}

func (s *workspaceAttachmentStorageProbe) DeleteKeys(ctx context.Context, keys []string) {
	s.deletedMu.Lock()
	s.deletedKeys = append(s.deletedKeys, keys...)
	s.deletedMu.Unlock()

	var workspaceExists bool
	if err := s.pool.QueryRow(ctx, `
SELECT EXISTS (SELECT 1 FROM workspace WHERE id = $1)
`, s.workspaceID).Scan(&workspaceExists); err != nil {
		s.observationMu.Lock()
		s.observationErr = err
		s.observationMu.Unlock()
		return
	}
	var attachmentCount int
	if err := s.pool.QueryRow(ctx, `
SELECT count(*) FROM attachment WHERE workspace_id = $1
`, s.workspaceID).Scan(&attachmentCount); err != nil {
		s.observationMu.Lock()
		s.observationErr = err
		s.observationMu.Unlock()
		return
	}
	s.observationMu.Lock()
	s.observedAfterCommit = !workspaceExists && attachmentCount == 0
	s.observationMu.Unlock()
}

func (s *workspaceAttachmentStorageProbe) keys() []string {
	s.deletedMu.Lock()
	defer s.deletedMu.Unlock()
	return append([]string(nil), s.deletedKeys...)
}

func (s *workspaceAttachmentStorageProbe) committedObservation() (bool, error) {
	s.observationMu.Lock()
	defer s.observationMu.Unlock()
	return s.observedAfterCommit, s.observationErr
}

type workspaceAttachmentRollbackTx struct {
	pgx.Tx
}

func (tx workspaceAttachmentRollbackTx) Commit(ctx context.Context) error {
	_ = tx.Tx.Rollback(ctx)
	return errors.New("forced workspace attachment cleanup commit failure")
}

type workspaceAttachmentRollbackTxStarter struct {
	pool *pgxpool.Pool
}

func (s workspaceAttachmentRollbackTxStarter) Begin(ctx context.Context) (pgx.Tx, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return nil, err
	}
	return workspaceAttachmentRollbackTx{Tx: tx}, nil
}

func TestDeleteWorkspaceDeletesAllAttachmentObjectsAfterCommit(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	fixture := newWorkspaceAttachmentCleanupFixture(t)
	originalStorage := testHandler.Storage
	probe := &workspaceAttachmentStorageProbe{pool: testPool, workspaceID: fixture.workspaceID}
	testHandler.Storage = probe
	t.Cleanup(func() { testHandler.Storage = originalStorage })

	request := newRequest(http.MethodDelete, "/api/workspaces/"+fixture.workspaceID, nil)
	request = withURLParam(request, "id", fixture.workspaceID)
	recorder := httptest.NewRecorder()
	testHandler.DeleteWorkspace(recorder, request)
	if recorder.Code != http.StatusNoContent {
		t.Fatalf("DeleteWorkspace: expected 204, got %d: %s", recorder.Code, recorder.Body.String())
	}

	got := probe.keys()
	sort.Strings(got)
	want := append([]string(nil), fixture.attachmentKeys...)
	sort.Strings(want)
	if len(got) != len(want) {
		t.Fatalf("deleted storage keys = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("deleted storage keys = %v, want %v", got, want)
		}
	}
	if containsWorkspaceStorageKey(got, fixture.neighborKey) {
		t.Fatalf("deleted storage keys included neighbor workspace object %q: %v", fixture.neighborKey, got)
	}
	if committed, err := probe.committedObservation(); err != nil {
		t.Fatalf("storage commit observation: %v", err)
	} else if !committed {
		t.Fatal("storage deletion was not observed after the workspace transaction committed")
	}

	var neighborCount int
	if err := testPool.QueryRow(context.Background(), `
SELECT count(*) FROM attachment WHERE workspace_id = $1
`, fixture.neighborID).Scan(&neighborCount); err != nil {
		t.Fatalf("count neighbor attachments: %v", err)
	}
	if neighborCount != 1 {
		t.Fatalf("neighbor attachment rows = %d, want 1", neighborCount)
	}
}

func TestDeleteWorkspaceDoesNotDeleteAttachmentObjectsWhenCommitRollsBack(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	fixture := newWorkspaceAttachmentCleanupFixture(t)
	originalStorage := testHandler.Storage
	probe := &workspaceAttachmentStorageProbe{pool: testPool, workspaceID: fixture.workspaceID}
	testHandler.Storage = probe
	t.Cleanup(func() { testHandler.Storage = originalStorage })

	originalTxStarter := testHandler.TxStarter
	testHandler.TxStarter = workspaceAttachmentRollbackTxStarter{pool: testPool}
	t.Cleanup(func() { testHandler.TxStarter = originalTxStarter })

	request := newRequest(http.MethodDelete, "/api/workspaces/"+fixture.workspaceID, nil)
	request = withURLParam(request, "id", fixture.workspaceID)
	recorder := httptest.NewRecorder()
	testHandler.DeleteWorkspace(recorder, request)
	if recorder.Code != http.StatusInternalServerError {
		t.Fatalf("DeleteWorkspace with forced commit failure: expected 500, got %d: %s", recorder.Code, recorder.Body.String())
	}
	if got := probe.keys(); len(got) != 0 {
		t.Fatalf("storage deletion ran after rollback: %v", got)
	}

	var workspaceExists bool
	if err := testPool.QueryRow(context.Background(), `
SELECT EXISTS (SELECT 1 FROM workspace WHERE id = $1)
`, fixture.workspaceID).Scan(&workspaceExists); err != nil {
		t.Fatalf("check rolled-back workspace: %v", err)
	}
	if !workspaceExists {
		t.Fatal("workspace disappeared despite a failed commit")
	}
	var attachmentCount int
	if err := testPool.QueryRow(context.Background(), `
SELECT count(*) FROM attachment WHERE workspace_id = $1
`, fixture.workspaceID).Scan(&attachmentCount); err != nil {
		t.Fatalf("check rolled-back attachments: %v", err)
	}
	if attachmentCount != 7 {
		t.Fatalf("workspace attachment rows after rollback = %d, want 7", attachmentCount)
	}
}

func containsWorkspaceStorageKey(values []string, wanted string) bool {
	for _, value := range values {
		if value == wanted {
			return true
		}
	}
	return false
}
