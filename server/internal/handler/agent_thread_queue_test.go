package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestPrioritizeAgentThreadTaskSelectsQueuedContinuationAndCurrentExecution(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Agent thread steer fixture", nil)
	issueID := dbfx.Issue(t, "Agent thread steer issue")
	runtimeID := handlerTestRuntimeID(t)
	rootID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID,
		"issue_id":   issueID,
		"status":     "running",
		"session_id": "provider-steer-thread",
		"started_at": testutil.Raw("now()"),
	})
	firstQueuedID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID,
		"issue_id":   issueID,
		"status":     "queued",
		"priority":   4,
		"session_id": "provider-steer-thread",
		"context": map[string]any{
			"agent_thread_parent_task_id": rootID,
			"agent_thread_message":        "first follow-up",
		},
	})
	selectedID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID,
		"issue_id":   issueID,
		"status":     "deferred",
		"priority":   4,
		"session_id": "provider-steer-thread",
		"fire_at":    testutil.Raw("now()"),
		"context": map[string]any{
			"agent_thread_parent_task_id": firstQueuedID,
			"agent_thread_message":        "selected follow-up",
		},
	})

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/tasks/"+rootID+"/agent-thread/queued-tasks/"+selectedID+"/prioritize", nil)
	req = withURLParams(req, "taskId", rootID, "queuedTaskId", selectedID)
	testHandler.PrioritizeAgentThreadTask(w, withChatTestWorkspaceCtx(t, req))

	if w.Code != http.StatusOK {
		t.Fatalf("prioritize Agent thread task: status=%d body=%s", w.Code, w.Body.String())
	}
	var receipt struct {
		TaskID       string `json:"task_id"`
		ActiveTaskID string `json:"active_task_id"`
	}
	if err := json.NewDecoder(w.Body).Decode(&receipt); err != nil {
		t.Fatalf("decode prioritize receipt: %v", err)
	}
	if receipt.TaskID != selectedID || receipt.ActiveTaskID != rootID {
		t.Fatalf("prioritize receipt = %#v, want selected=%s active=%s", receipt, selectedID, rootID)
	}

	var selectedStatus, firstStatus string
	var selectedPriority, firstPriority int32
	dbfx.QueryRow(t, `SELECT status, priority FROM agent_task_queue WHERE id = $1`, selectedID).
		Scan(&selectedStatus, &selectedPriority)
	dbfx.QueryRow(t, `SELECT status, priority FROM agent_task_queue WHERE id = $1`, firstQueuedID).
		Scan(&firstStatus, &firstPriority)
	if selectedStatus != "queued" || selectedPriority != 5 {
		t.Fatalf("selected task state = %s/%d, want queued/5", selectedStatus, selectedPriority)
	}
	if firstStatus != "deferred" || firstPriority > 4 {
		t.Fatalf("prior queued task state = %s/%d, want deferred/<=4", firstStatus, firstPriority)
	}
}

func TestPrioritizeAgentThreadTaskRejectsOrdinaryChatTask(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Agent thread steer chat boundary", nil)
	chatID := dbfx.ChatSession(t, agentID, testutil.Cols{})
	runtimeID := handlerTestRuntimeID(t)
	rootID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":      runtimeID,
		"chat_session_id": chatID,
		"status":          "running",
		"session_id":      "chat-provider-thread",
	})
	queuedID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":      runtimeID,
		"chat_session_id": chatID,
		"status":          "queued",
		"session_id":      "chat-provider-thread",
	})

	w := httptest.NewRecorder()
	req := newRequest("POST", "/api/tasks/"+rootID+"/agent-thread/queued-tasks/"+queuedID+"/prioritize", nil)
	req = withURLParams(req, "taskId", rootID, "queuedTaskId", queuedID)
	testHandler.PrioritizeAgentThreadTask(w, withChatTestWorkspaceCtx(t, req))

	if w.Code != http.StatusNotFound {
		t.Fatalf("ordinary Chat task steered through Agent thread: status=%d body=%s", w.Code, w.Body.String())
	}
}
