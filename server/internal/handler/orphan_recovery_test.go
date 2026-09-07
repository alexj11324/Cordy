package handler

import (
	"context"
	"net/http"
	"strconv"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func TestRecoverOrphansHTTPPreservesSavedResultAndRecoversOtherTask(t *testing.T) {
	runtimeID := dbfx.Insert(t, "agent_runtime", testutil.Cols{"workspace_id": testWorkspaceID, "owner_id": testUserID, "name": "pending-terminal-http", "runtime_mode": "cloud", "provider": "codex", "status": "online", "device_info": "", "metadata": "{}"})
	agentID := dbfx.Agent(t, "pending-terminal-http", runtimeID)
	seed := func() db.AgentTaskQueue {
		issueID := dbfx.Issue(t, "pending terminal recovery", testutil.Cols{"status": "in_progress", "executor_type": "agent", "executor_id": agentID})
		taskID := dbfx.Task(t, agentID, testutil.Cols{"runtime_id": runtimeID, "issue_id": issueID, "status": "running", "dispatched_at": time.Now().Add(-time.Minute), "started_at": time.Now().Add(-time.Minute), "attempt": 0, "max_attempts": 1})
		dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE parent_task_id=$1`, taskID)
		task, err := testHandler.Queries.GetAgentTask(context.Background(), parseUUID(taskID))
		if err != nil {
			t.Fatal(err)
		}
		return task
	}
	saved, orphan := seed(), seed()
	path := "/api/daemon/runtimes/" + runtimeID + "/recover-orphans"
	call := func(body any) *testutil.Response {
		request := withURLParam(newDaemonTokenRequest(http.MethodPost, path, body, testWorkspaceID, "outbox-restart-daemon"), "runtimeId", runtimeID)
		return testutil.Call(t, testHandler.RecoverOrphanedTasks, request)
	}
	for _, bad := range []map[string]any{
		{"task_id": "malformed", "claim_fence": "1"},
		{"task_id": uuidToString(saved.ID), "claim_fence": "1.2"},
		{"task_id": uuidToString(saved.ID), "claim_fence": 123},
	} {
		call(map[string]any{"pending_terminal_reports": []any{bad}}).Want(http.StatusBadRequest)
	}
	var response struct {
		Orphaned int `json:"orphaned"`
		Retried  int `json:"retried"`
	}
	call(protocol.RecoverOrphansRequest{PendingTerminalReports: []protocol.PendingTerminalReport{{TaskID: uuidToString(saved.ID), ClaimFence: strconv.FormatInt(service.TaskClaimFence(saved), 10)}}}).Want(http.StatusOK).JSON(&response)
	if response.Orphaned != 1 || response.Retried != 1 {
		t.Fatalf("recovery response=%+v; want one unrelated orphan and its normal retry", response)
	}
	for _, tc := range []struct {
		task db.AgentTaskQueue
		want string
	}{{saved, "running"}, {orphan, "failed"}} {
		var status string
		dbfx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id=$1`, tc.task.ID).Scan(&status)
		if status != tc.want {
			t.Fatalf("task %s status=%s want %s", uuidToString(tc.task.ID), status, tc.want)
		}
	}
	if got := dbfx.Count(t, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`, orphan.ID); got != 1 {
		t.Fatalf("unrelated orphan acquired %d retries, want 1", got)
	}
	if got := dbfx.Count(t, `SELECT count(*) FROM agent_task_queue WHERE parent_task_id=$1`, saved.ID); got != 0 {
		t.Fatalf("saved result acquired %d provider retry tasks", got)
	}
}
