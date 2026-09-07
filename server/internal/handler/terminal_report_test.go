package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/service"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/dbid"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

func seedTerminalReportTask(t *testing.T) db.AgentTaskQueue {
	t.Helper()
	runtimeID := handlerTestRuntimeID(t)
	agentID := dbfx.Agent(t, "terminal-report-agent", runtimeID)
	issueID := dbfx.Issue(t, "terminal report issue", testutil.Cols{"status": "in_progress", "executor_type": "agent", "executor_id": agentID})
	taskID := dbfx.Task(t, agentID, testutil.Cols{"runtime_id": runtimeID, "issue_id": issueID, "status": "running", "dispatched_at": time.Now().Add(-time.Second), "started_at": time.Now(), "attempt": 0, "max_attempts": 1})
	dbfx.Cleanup(t, `DELETE FROM comment WHERE source_task_id=$1`, taskID)
	dbfx.Cleanup(t, `DELETE FROM terminal_report_receipt WHERE task_id=$1`, taskID)
	task, err := testHandler.Queries.GetAgentTask(context.Background(), parseUUID(taskID))
	if err != nil {
		t.Fatal(err)
	}
	return task
}

func terminalReportBody(t *testing.T, kind string, task db.AgentTaskQueue, body map[string]any) (map[string]any, protocol.TerminalReportIdentity) {
	t.Helper()
	raw, err := json.Marshal(body)
	if err != nil {
		t.Fatal(err)
	}
	digest, err := protocol.TerminalReportDigest(kind, raw)
	if err != nil {
		t.Fatal(err)
	}
	identity := protocol.TerminalReportIdentity{ReportID: uuidToString(dbid.NewV7()), ClaimFence: strconv.FormatInt(service.TaskClaimFence(task), 10), PayloadSHA256: digest}
	body["terminal_report"] = identity
	return body, identity
}

func TestTerminalReportHTTPReplayPreservesOriginalAckAndSanitization(t *testing.T) {
	for _, tc := range []struct{ name, kind, output, status string }{
		{"complete", "complete", "done\x00 summary text", "completed"},
		{"fail", "fail", "worker\x00 stopped", "failed"},
		{"context exhaustion", "complete", "Prompt is too long · the request is ~274931 tokens (limit 200000) but this conversation is only ~1597 tokens — the rest is system prompt, tool definitions, and attachment content. A single-exchange conversation cannot be compacted; reduce attached files/tools or start with less context.", "failed"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			task := seedTerminalReportTask(t)
			body := map[string]any{"work_dir": "/tmp/work\x00dir"}
			endpoint := testHandler.CompleteTask
			if tc.kind == "complete" {
				body["output"] = tc.output
			} else {
				body["error"] = tc.output
				body["failure_reason"] = "agent_error"
				endpoint = testHandler.FailTask
			}
			body, identity := terminalReportBody(t, tc.kind, task, body)
			taskID := uuidToString(task.ID)
			path := "/api/daemon/tasks/" + taskID + "/" + tc.kind
			var first, repeat protocol.TerminalReportAck
			// Discarding this first response models a connection loss after database commit.
			testutil.Call(t, endpoint, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusOK).JSON(&first)
			testutil.Call(t, endpoint, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusOK).JSON(&repeat)
			if first != repeat || first.TerminalReportIdentity != identity || first.TaskID != taskID || first.Status != "accepted" || first.TaskStatus != tc.status {
				t.Fatalf("acks: first=%+v repeat=%+v", first, repeat)
			}
			if got := dbfx.Count(t, `SELECT count(*) FROM comment WHERE source_task_id=$1`, task.ID); got != 1 {
				t.Fatalf("response replay produced %d outcome comments", got)
			}
			var workDir string
			dbfx.QueryRow(t, `SELECT work_dir FROM agent_task_queue WHERE id=$1`, task.ID).Scan(&workDir)
			if workDir != "/tmp/workdir" {
				t.Fatalf("sanitization lost: %q", workDir)
			}
			// A changed body with the old digest is malformed, not an accepted replay.
			body["work_dir"] = "/tmp/changed"
			testutil.Call(t, endpoint, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusBadRequest)
		})
	}
}

func TestTerminalReportHTTPRejectsInvalidIdentityAndStaleClaim(t *testing.T) {
	task := seedTerminalReportTask(t)
	taskID := uuidToString(task.ID)
	path := "/api/daemon/tasks/" + taskID + "/complete"
	body, identity := terminalReportBody(t, "complete", task, map[string]any{"output": "done"})
	for _, tc := range []struct {
		name   string
		change func(*protocol.TerminalReportIdentity)
	}{
		{"UUID", func(i *protocol.TerminalReportIdentity) { i.ReportID = "invalid" }},
		{"unsafe number", func(i *protocol.TerminalReportIdentity) { i.ClaimFence = "1.5" }},
		{"empty fence", func(i *protocol.TerminalReportIdentity) { i.ClaimFence = "" }},
		{"hash", func(i *protocol.TerminalReportIdentity) { i.PayloadSHA256 = "abc" }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			bad := identity
			tc.change(&bad)
			body["terminal_report"] = bad
			testutil.Call(t, testHandler.CompleteTask, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusBadRequest)
		})
	}
	old := identity
	old.ClaimFence = strconv.FormatInt(service.TaskClaimFence(task)-32, 10)
	body["terminal_report"] = old
	testutil.Call(t, testHandler.CompleteTask, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusConflict)
	body["terminal_report"] = identity
	var ack protocol.TerminalReportAck
	testutil.Call(t, testHandler.CompleteTask, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusOK).JSON(&ack)
	conflict := identity
	conflict.ReportID = uuidToString(dbid.NewV7())
	body["terminal_report"] = conflict
	testutil.Call(t, testHandler.CompleteTask, daemonTaskRequest(t, path, taskID, body)).Want(http.StatusConflict)
	if got := dbfx.Count(t, `SELECT count(*) FROM terminal_report_receipt WHERE task_id=$1`, task.ID); got != 1 {
		t.Fatalf("receipt count=%d", got)
	}
}

func TestTerminalReportClaimFenceIsDecimalString(t *testing.T) {
	task := seedTerminalReportTask(t)
	response := taskToResponse(task, testWorkspaceID)
	raw, err := json.Marshal(response)
	if err != nil {
		t.Fatal(err)
	}
	var body map[string]any
	if err := json.Unmarshal(raw, &body); err != nil {
		t.Fatal(err)
	}
	if got, ok := body["claim_fence"].(string); !ok || got != strconv.FormatInt(service.TaskClaimFence(task), 10) {
		t.Fatalf("claim_fence=%v; want decimal text", body["claim_fence"])
	}
	if strings.Contains(string(raw), "terminal_report") {
		t.Fatal("ordinary task response unexpectedly contains a receipt")
	}
}
