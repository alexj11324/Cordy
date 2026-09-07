package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestAgentThreadContinuationIdempotencyIsBoundToParent(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	agentID := createHandlerTestAgent(t, "Agent thread idempotency scope", nil)
	parentA := createAutomationRunOnlyTask(t, agentID)
	parentB := createAutomationRunOnlyTask(t, agentID)
	dbfx.Exec(t, `
		UPDATE agent_task_queue
		SET status = 'completed', session_id = $2, completed_at = now()
		WHERE id = $1
	`, parentA, "provider-session-a")
	dbfx.Exec(t, `
		UPDATE agent_task_queue
		SET status = 'completed', session_id = $2, completed_at = now()
		WHERE id = $1
	`, parentB, "provider-session-b")

	var runA, runB string
	dbfx.QueryRow(t, `SELECT automation_run_id::text FROM agent_task_queue WHERE id = $1`, parentA).Scan(&runA)
	dbfx.QueryRow(t, `SELECT automation_run_id::text FROM agent_task_queue WHERE id = $1`, parentB).Scan(&runB)
	if runA == runB {
		t.Fatalf("fixture automation roots unexpectedly share automation_run_id %q", runA)
	}

	continueTask := func(parentID, content string) (string, string) {
		t.Helper()
		w := httptest.NewRecorder()
		req := withURLParam(
			newRequest(http.MethodPost, "/api/tasks/"+parentID+"/agent-thread/continue", map[string]any{"content": content}),
			"taskId", parentID,
		)
		req.Header.Set("Idempotency-Key", "same-key-across-parents")
		testHandler.ContinueAgentThread(w, withChatTestWorkspaceCtx(t, req))
		if w.Code != http.StatusOK {
			t.Fatalf("continue parent %s: status=%d body=%s", parentID, w.Code, w.Body.String())
		}
		var body map[string]string
		if err := json.NewDecoder(w.Body).Decode(&body); err != nil {
			t.Fatalf("decode continuation receipt for %s: %v", parentID, err)
		}
		return body["status"], body["continuation_task_id"]
	}

	statusA, childA := continueTask(parentA, "follow up parent A")
	if statusA != "queued" || childA == "" {
		t.Fatalf("first continuation = status %q child %q, want queued child", statusA, childA)
	}
	replayStatus, replayChild := continueTask(parentA, "follow up parent A")
	if replayStatus != "coalesced" || replayChild != childA {
		t.Fatalf("same-parent replay = status %q child %q, want coalesced %q", replayStatus, replayChild, childA)
	}

	statusB, childB := continueTask(parentB, "follow up parent B")
	if statusB != "queued" || childB == "" || childB == childA {
		t.Fatalf("different-parent continuation = status %q child %q, want a new queued child distinct from %q", statusB, childB, childA)
	}

	assertContinuation := func(childID, wantParent, wantMessage string) {
		t.Helper()
		var gotParent, gotMessage string
		dbfx.QueryRow(t, `
			SELECT context->>'agent_thread_parent_task_id', context->>'agent_thread_message'
			FROM agent_task_queue WHERE id = $1
		`, childID).Scan(&gotParent, &gotMessage)
		if gotParent != wantParent || gotMessage != wantMessage {
			t.Fatalf("child %s lineage = (%q, %q), want (%q, %q)", childID, gotParent, gotMessage, wantParent, wantMessage)
		}
	}
	assertContinuation(childA, parentA, "follow up parent A")
	assertContinuation(childB, parentB, "follow up parent B")
}
