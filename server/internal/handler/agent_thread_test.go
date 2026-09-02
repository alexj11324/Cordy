package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

func TestAgentThreadContinuationIsTaskScopedAndIdempotent(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Agent thread fixture", nil)
	issueID := dbfx.Issue(t, "Agent thread issue")
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   handlerTestRuntimeID(t),
		"issue_id":     issueID,
		"status":       "completed",
		"session_id":   "provider-thread-1",
		"completed_at": testutil.Raw("now()"),
	})

	get := httptest.NewRecorder()
	getRequest := withURLParam(newRequest("GET", "/api/tasks/"+parentID+"/agent-thread", nil), "taskId", parentID)
	testHandler.GetAgentThread(get, withChatTestWorkspaceCtx(t, getRequest))
	if get.Code != http.StatusOK {
		t.Fatalf("GET Agent thread: status=%d body=%s", get.Code, get.Body.String())
	}
	var envelope struct {
		CanContinue bool `json:"can_continue"`
	}
	if err := json.NewDecoder(get.Body).Decode(&envelope); err != nil || !envelope.CanContinue {
		t.Fatalf("GET Agent thread continuation state: %#v err=%v", envelope, err)
	}

	continueOnce := func(content string) (int, map[string]string) {
		w := httptest.NewRecorder()
		req := withURLParam(newRequest("POST", "/api/tasks/"+parentID+"/agent-thread/continue", map[string]any{"content": content}), "taskId", parentID)
		req.Header.Set("Idempotency-Key", "agent-thread-receipt-1")
		testHandler.ContinueAgentThread(w, withChatTestWorkspaceCtx(t, req))
		body := map[string]string{}
		_ = json.NewDecoder(w.Body).Decode(&body)
		return w.Code, body
	}

	status, first := continueOnce("continue this exact task")
	if status != http.StatusOK || first["status"] != "queued" || first["continuation_task_id"] == "" {
		t.Fatalf("first continuation: status=%d body=%#v", status, first)
	}
	status, replay := continueOnce("continue this exact task")
	if status != http.StatusOK || replay["status"] != "coalesced" || replay["continuation_task_id"] != first["continuation_task_id"] {
		t.Fatalf("idempotent replay: status=%d body=%#v first=%#v", status, replay, first)
	}
	status, conflict := continueOnce("different content")
	if status != http.StatusConflict || conflict["error"] != "agent_thread_idempotency_conflict" {
		t.Fatalf("idempotency conflict: status=%d body=%#v", status, conflict)
	}

	var chatSessionID, automationRunID *string
	var storedContent, storedParent string
	dbfx.QueryRow(t, `
		SELECT chat_session_id, automation_run_id,
		       context->>'agent_thread_message', context->>'agent_thread_parent_task_id'
		FROM agent_task_queue WHERE id = $1
	`, first["continuation_task_id"]).Scan(&chatSessionID, &automationRunID, &storedContent, &storedParent)
	if chatSessionID != nil || automationRunID != nil || storedContent != "continue this exact task" || storedParent != parentID {
		t.Fatalf("continuation crossed task boundary: chat=%v automation=%v content=%q parent=%q", chatSessionID, automationRunID, storedContent, storedParent)
	}
}

func TestAgentThreadRejectsOrdinaryChatTask(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Agent thread chat boundary", nil)
	chatID := dbfx.ChatSession(t, agentID, testutil.Cols{})
	taskID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":      handlerTestRuntimeID(t),
		"chat_session_id": chatID,
		"status":          "completed",
		"session_id":      "ordinary-chat-session",
		"completed_at":    testutil.Raw("now()"),
	})
	w := httptest.NewRecorder()
	request := withURLParam(newRequest("GET", "/api/tasks/"+taskID+"/agent-thread", nil), "taskId", taskID)
	testHandler.GetAgentThread(w, withChatTestWorkspaceCtx(t, request))
	if w.Code != http.StatusNotFound {
		t.Fatalf("ordinary Chat task exposed as task conversation: status=%d body=%s", w.Code, w.Body.String())
	}
}
