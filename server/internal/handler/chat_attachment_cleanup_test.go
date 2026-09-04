package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sort"
	"sync"
	"testing"
)

type attachmentCleanupStorage struct {
	mockStorage

	deletedMu   sync.Mutex
	deletedKeys []string
}

func (s *attachmentCleanupStorage) DeleteKeys(_ context.Context, keys []string) {
	s.deletedMu.Lock()
	defer s.deletedMu.Unlock()
	s.deletedKeys = append(s.deletedKeys, keys...)
}

func (s *attachmentCleanupStorage) keys() []string {
	s.deletedMu.Lock()
	defer s.deletedMu.Unlock()
	return append([]string(nil), s.deletedKeys...)
}

func TestDeleteChatSessionDeletesAttachmentObjects(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	originalStorage := testHandler.Storage
	store := &attachmentCleanupStorage{}
	testHandler.Storage = store
	t.Cleanup(func() { testHandler.Storage = originalStorage })

	agentID := createHandlerTestAgent(t, "ChatAttachmentCleanupAgent", []byte("[]"))
	sessionID := createHandlerTestChatSession(t, agentID)

	var messageID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO chat_message (chat_session_id, role, content)
		VALUES ($1, 'assistant', 'attachment cleanup test')
		RETURNING id
	`, sessionID).Scan(&messageID); err != nil {
		t.Fatalf("create chat message: %v", err)
	}

	if _, err := testPool.Exec(ctx, `
		INSERT INTO attachment (
			workspace_id, chat_session_id, uploader_type, uploader_id,
			filename, url, content_type, size_bytes
		)
		VALUES ($1, $2, 'member', $3, 'session.txt',
		        'https://cdn.example.com/chat-session-only', 'text/plain', 1)
	`, testWorkspaceID, sessionID, testUserID); err != nil {
		t.Fatalf("create session attachment: %v", err)
	}
	if _, err := testPool.Exec(ctx, `
		INSERT INTO attachment (
			workspace_id, chat_session_id, chat_message_id, uploader_type, uploader_id,
			filename, url, content_type, size_bytes
		)
		VALUES ($1, $2, $3, 'member', $4, 'message.txt',
		        'https://cdn.example.com/chat-message-owned', 'text/plain', 1)
	`, testWorkspaceID, sessionID, messageID, testUserID); err != nil {
		t.Fatalf("create message attachment: %v", err)
	}

	req := httptest.NewRequest(http.MethodDelete, "/api/chat/sessions/"+sessionID, nil)
	req.Header.Set("X-User-ID", testUserID)
	req = withURLParam(req, "sessionId", sessionID)
	req = withChatTestWorkspaceCtx(t, req)
	w := httptest.NewRecorder()
	testHandler.DeleteChatSession(w, req)
	if w.Code != http.StatusNoContent {
		t.Fatalf("DeleteChatSession: expected 204, got %d: %s", w.Code, w.Body.String())
	}

	got := store.keys()
	sort.Strings(got)
	want := []string{"chat-message-owned", "chat-session-only"}
	if len(got) != len(want) {
		t.Fatalf("deleted storage keys = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("deleted storage keys = %v, want %v", got, want)
			break
		}
	}
}

func TestDeleteAgentRuntimeDeletesSystemChatAttachmentObjects(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	originalStorage := testHandler.Storage
	store := &attachmentCleanupStorage{}
	testHandler.Storage = store
	t.Cleanup(func() { testHandler.Storage = originalStorage })

	runtimeID := createCascadeFixtureRuntime(t, ctx, "System Chat Attachment Cleanup Runtime")
	var agentID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent (
			workspace_id, name, runtime_mode, runtime_config, runtime_id,
			visibility, max_concurrent_tasks, owner_id, kind, system_key
		)
		VALUES ($1, 'system attachment cleanup agent', 'cloud', '{}'::jsonb, $2,
		        'private', 1, $3, 'system', 'agent_builder:' || gen_random_uuid()::text)
		RETURNING id
	`, testWorkspaceID, runtimeID, testUserID).Scan(&agentID); err != nil {
		t.Fatalf("create system agent: %v", err)
	}

	var sessionID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO chat_session (
			workspace_id, agent_id, creator_id, title, status, explicitly_created_at
		)
		VALUES ($1, $2, $3, 'system attachment cleanup session', 'active', now())
		RETURNING id
	`, testWorkspaceID, agentID, testUserID).Scan(&sessionID); err != nil {
		t.Fatalf("create system chat session: %v", err)
	}
	if _, err := testPool.Exec(ctx, `
		INSERT INTO attachment (
			workspace_id, chat_session_id, uploader_type, uploader_id,
			filename, url, content_type, size_bytes
		)
		VALUES ($1, $2, 'agent', $3, 'runtime.txt',
		        'https://cdn.example.com/runtime-system-chat', 'text/plain', 1)
	`, testWorkspaceID, sessionID, agentID); err != nil {
		t.Fatalf("create system chat attachment: %v", err)
	}

	req := newRequest(http.MethodDelete, "/api/runtimes/"+runtimeID, nil)
	req = withURLParam(req, "runtimeId", runtimeID)
	w := httptest.NewRecorder()
	testHandler.DeleteAgentRuntime(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("DeleteAgentRuntime: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	got := store.keys()
	want := []string{"runtime-system-chat"}
	if len(got) != 1 || got[0] != want[0] {
		t.Fatalf("deleted storage keys = %v, want %v", got, want)
	}
}
