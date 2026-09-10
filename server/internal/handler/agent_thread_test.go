package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/middleware"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	agentpkg "github.com/orvilo-ai/orvilo/server/pkg/agent"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/dbid"
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

func TestAgentThreadContinuationBindsOwnedAttachmentsAndRejectsForeignRows(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Agent thread attachment fixture", nil)
	issueID := dbfx.Issue(t, "Agent thread attachment issue")
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   handlerTestRuntimeID(t),
		"issue_id":     issueID,
		"status":       "completed",
		"session_id":   "agent-thread-attachment-provider",
		"completed_at": testutil.Raw("now()"),
	})
	ownedID := dbfx.Insert(t, "attachment", testutil.Cols{
		"workspace_id":  testWorkspaceID,
		"uploader_type": "member",
		"uploader_id":   testUserID,
		"filename":      "follow-up.png",
		"url":           "https://cdn.example/follow-up.png",
		"content_type":  "image/png",
		"size_bytes":    128,
	})
	foreignOwnerID := createPlainMember(t, "agent-thread-attachment-foreign@example.com")
	foreignOwnerAttachmentID := dbfx.Insert(t, "attachment", testutil.Cols{
		"workspace_id":  testWorkspaceID,
		"uploader_type": "member",
		"uploader_id":   foreignOwnerID,
		"filename":      "private.png",
		"url":           "https://cdn.example/private.png",
		"content_type":  "image/png",
		"size_bytes":    64,
	})

	continueTask := func(content, key string, attachmentIDs []string) (int, map[string]string) {
		t.Helper()
		w := httptest.NewRecorder()
		req := withURLParam(newRequest("POST", "/api/tasks/"+parentID+"/agent-thread/continue", map[string]any{
			"content":        content,
			"attachment_ids": attachmentIDs,
		}), "taskId", parentID)
		req.Header.Set("Idempotency-Key", key)
		testHandler.ContinueAgentThread(w, withChatTestWorkspaceCtx(t, req))
		body := map[string]string{}
		_ = json.NewDecoder(w.Body).Decode(&body)
		return w.Code, body
	}

	status, receipt := continueTask("", "agent-thread-attachment-owned", []string{ownedID})
	if status != http.StatusOK || receipt["continuation_task_id"] == "" {
		t.Fatalf("owned attachment continuation: status=%d body=%#v", status, receipt)
	}
	childID := receipt["continuation_task_id"]
	var boundTaskID *string
	if err := testPool.QueryRow(t.Context(), `SELECT task_id::text FROM attachment WHERE id = $1`, ownedID).Scan(&boundTaskID); err != nil {
		t.Fatalf("load bound attachment: %v", err)
	}
	if boundTaskID == nil || *boundTaskID != childID {
		t.Fatalf("owned attachment task_id = %v, want %s", boundTaskID, childID)
	}

	status, rejected := continueTask("read this private image", "agent-thread-attachment-foreign", []string{foreignOwnerAttachmentID})
	if status != http.StatusBadRequest || rejected["error"] == "" {
		t.Fatalf("foreign attachment continuation: status=%d body=%#v", status, rejected)
	}
	var foreignTaskID *string
	if err := testPool.QueryRow(t.Context(), `SELECT task_id::text FROM attachment WHERE id = $1`, foreignOwnerAttachmentID).Scan(&foreignTaskID); err != nil {
		t.Fatalf("load rejected attachment: %v", err)
	}
	if foreignTaskID != nil {
		t.Fatalf("foreign attachment was bound to %v", foreignTaskID)
	}
}

func TestAgentThreadSupportsRunOnlyAutomationTasks(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Automation Agent thread fixture", nil)
	parentID := createAutomationRunOnlyTask(t, agentID)
	if _, err := testPool.Exec(t.Context(), `
		UPDATE agent_task_queue
		SET status = 'completed', session_id = 'automation-provider-thread-1', completed_at = now()
		WHERE id = $1
	`, parentID); err != nil {
		t.Fatalf("prepare automation task: %v", err)
	}

	get := httptest.NewRecorder()
	getRequest := withURLParam(newRequest("GET", "/api/tasks/"+parentID+"/agent-thread", nil), "taskId", parentID)
	testHandler.GetAgentThread(get, withChatTestWorkspaceCtx(t, getRequest))
	if get.Code != http.StatusOK {
		t.Fatalf("GET automation Agent thread: status=%d body=%s", get.Code, get.Body.String())
	}

	continueTask := func(taskID, content, key string) string {
		t.Helper()
		w := httptest.NewRecorder()
		req := withURLParam(newRequest("POST", "/api/tasks/"+taskID+"/agent-thread/continue", map[string]any{"content": content}), "taskId", taskID)
		req.Header.Set("Idempotency-Key", key)
		testHandler.ContinueAgentThread(w, withChatTestWorkspaceCtx(t, req))
		var body map[string]string
		_ = json.NewDecoder(w.Body).Decode(&body)
		if w.Code != http.StatusOK || body["continuation_task_id"] == "" {
			t.Fatalf("continue automation Agent thread: status=%d body=%#v", w.Code, body)
		}
		return body["continuation_task_id"]
	}

	firstChildID := continueTask(parentID, "follow up on this run", "automation-thread-1")
	dbfx.Exec(t, `UPDATE agent_task_queue SET priority = 5 WHERE id = $1`, firstChildID)
	secondChildID := continueTask(firstChildID, "steer the follow-up", "automation-thread-2")

	var issueID, chatSessionID, automationRunID *string
	var storedParent string
	var priority int
	dbfx.QueryRow(t, `
		SELECT issue_id, chat_session_id, automation_run_id,
		       context->>'agent_thread_parent_task_id', priority
		FROM agent_task_queue WHERE id = $1
	`, secondChildID).Scan(&issueID, &chatSessionID, &automationRunID, &storedParent, &priority)
	if issueID != nil || chatSessionID != nil || automationRunID != nil || storedParent != firstChildID {
		t.Fatalf("automation continuation escaped conversation-only lineage: issue=%v chat=%v automation=%v parent=%q", issueID, chatSessionID, automationRunID, storedParent)
	}
	if priority != 4 {
		t.Fatalf("normal follow-up inherited reserved Steer priority: got %d, want 4", priority)
	}

	getChild := httptest.NewRecorder()
	getChildRequest := withURLParam(newRequest("GET", "/api/tasks/"+secondChildID+"/agent-thread", nil), "taskId", secondChildID)
	testHandler.GetAgentThread(getChild, withChatTestWorkspaceCtx(t, getChildRequest))
	if getChild.Code != http.StatusOK {
		t.Fatalf("GET automation continuation Agent thread: status=%d body=%s", getChild.Code, getChild.Body.String())
	}
}

func TestAgentThreadAutomationContinuationRequiresAutomationWrite(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Automation Agent thread write gate", nil)
	parentID := createAutomationRunOnlyTask(t, agentID)
	if _, err := testPool.Exec(t.Context(), `
		UPDATE agent_task_queue
		SET status = 'completed', session_id = 'automation-write-gate-thread', completed_at = now()
		WHERE id = $1
	`, parentID); err != nil {
		t.Fatalf("prepare automation task: %v", err)
	}
	memberID := createPlainMember(t, "agent-thread-no-automation-write@example.com")
	member, err := testHandler.Queries.GetMemberByUserAndWorkspace(context.Background(), db.GetMemberByUserAndWorkspaceParams{
		UserID: util.MustParseUUID(memberID), WorkspaceID: util.MustParseUUID(testWorkspaceID),
	})
	if err != nil {
		t.Fatalf("load member: %v", err)
	}
	requestForMember := func(method, path string, body any) *http.Request {
		req := newRequestAs(memberID, method, path, body)
		return req.WithContext(middleware.SetMemberContext(req.Context(), testWorkspaceID, member))
	}

	assertDenied := func(taskID, key string) {
		t.Helper()
		get := httptest.NewRecorder()
		getRequest := withURLParam(requestForMember("GET", "/api/tasks/"+taskID+"/agent-thread", nil), "taskId", taskID)
		testHandler.GetAgentThread(get, getRequest)
		if get.Code != http.StatusOK {
			t.Fatalf("GET automation Agent thread: status=%d body=%s", get.Code, get.Body.String())
		}
		var state struct {
			CanContinue bool `json:"can_continue"`
		}
		if err := json.NewDecoder(get.Body).Decode(&state); err != nil || state.CanContinue {
			t.Fatalf("read-only member continuation state: %#v err=%v", state, err)
		}

		continued := httptest.NewRecorder()
		continueRequest := withURLParam(requestForMember("POST", "/api/tasks/"+taskID+"/agent-thread/continue", map[string]any{"content": "inherit someone else's tools"}), "taskId", taskID)
		continueRequest.Header.Set("Idempotency-Key", key)
		testHandler.ContinueAgentThread(continued, continueRequest)
		if continued.Code != http.StatusForbidden {
			t.Fatalf("non-writer continued Automation thread: status=%d body=%s", continued.Code, continued.Body.String())
		}
	}
	assertDenied(parentID, "automation-write-gate-run-only")

	automationID := dbfx.Insert(t, "automation", testutil.Cols{
		"workspace_id":    testWorkspaceID,
		"title":           "create-issue write gate",
		"executor_id":     agentID,
		"execution_mode":  "create_issue",
		"created_by_type": "member",
		"created_by_id":   testUserID,
	})
	issueID := dbfx.Issue(t, "Automation-origin issue", testutil.Cols{
		"origin_type": "automation",
		"origin_id":   automationID,
	})
	issueTaskID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   handlerTestRuntimeID(t),
		"issue_id":     issueID,
		"status":       "completed",
		"session_id":   "automation-issue-write-gate-thread",
		"completed_at": testutil.Raw("now()"),
	})
	assertDenied(issueTaskID, "automation-write-gate-create-issue")
}

func TestAutomationAgentThreadContinuationClaimRestoresCapabilitiesWithoutOriginalPrompt(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Automation continuation claim fixture", nil)
	var runtimeID string
	dbfx.QueryRow(t, `SELECT runtime_id FROM agent WHERE id = $1`, agentID).Scan(&runtimeID)
	projectID := dbfx.Project(t, "Automation continuation project", testutil.Cols{})
	automationID := dbfx.Insert(t, "automation", testutil.Cols{
		"workspace_id":    testWorkspaceID,
		"project_id":      projectID,
		"title":           "Automation continuation",
		"description":     "ORIGINAL AUTOMATION INSTRUCTIONS",
		"model":           "gpt-5.4-mini",
		"tools":           `{"memories":{"enabled":true}}`,
		"executor_id":     agentID,
		"execution_mode":  "run_only",
		"created_by_type": "member",
		"created_by_id":   testUserID,
	})
	runID := dbfx.Insert(t, "automation_run", testutil.Cols{
		"automation_id": automationID,
		"source":        "manual",
		"status":        "completed",
	})
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":        runtimeID,
		"automation_run_id": runID,
		"status":            "completed",
		"session_id":        "automation-claim-provider-thread",
		"completed_at":      testutil.Raw("now()"),
	})

	continued := httptest.NewRecorder()
	continueRequest := withURLParam(newRequest("POST", "/api/tasks/"+parentID+"/agent-thread/continue", map[string]any{"content": "only follow this new direction"}), "taskId", parentID)
	continueRequest.Header.Set("Idempotency-Key", "automation-claim-thread-1")
	testHandler.ContinueAgentThread(continued, withChatTestWorkspaceCtx(t, continueRequest))
	if continued.Code != http.StatusOK {
		t.Fatalf("continue automation Agent thread: status=%d body=%s", continued.Code, continued.Body.String())
	}
	var receipt map[string]string
	if err := json.NewDecoder(continued.Body).Decode(&receipt); err != nil || receipt["continuation_task_id"] == "" {
		t.Fatalf("decode continuation receipt: %#v err=%v", receipt, err)
	}
	continuationID := receipt["continuation_task_id"]
	continuationTask, err := testHandler.Queries.GetAgentTask(t.Context(), parseUUID(continuationID))
	if err != nil {
		t.Fatalf("load continuation task: %v", err)
	}
	if workspaceID := testHandler.TaskService.ResolveTaskWorkspaceID(t.Context(), continuationTask); workspaceID != testWorkspaceID {
		t.Fatalf("continuation workspace = %q, want %q", workspaceID, testWorkspaceID)
	}

	claim := httptest.NewRecorder()
	claimRequest := withURLParam(
		newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/claim", nil, testWorkspaceID, "test-daemon"),
		"runtimeId",
		runtimeID,
	)
	testHandler.ClaimTaskByRuntime(claim, claimRequest)
	if claim.Code != http.StatusOK {
		t.Fatalf("claim continuation: status=%d body=%s", claim.Code, claim.Body.String())
	}
	var response struct {
		Task *struct {
			ID                    string         `json:"id"`
			AutomationID          string         `json:"automation_id"`
			AutomationDescription string         `json:"automation_description"`
			AgentThreadMessage    string         `json:"agent_thread_message"`
			AgentThreadRootTaskID string         `json:"agent_thread_root_task_id"`
			ThreadName            string         `json:"thread_name"`
			ProjectID             string         `json:"project_id"`
			Agent                 *TaskAgentData `json:"agent"`
		} `json:"task"`
	}
	if err := json.NewDecoder(claim.Body).Decode(&response); err != nil || response.Task == nil {
		t.Fatalf("decode continuation claim: task=%#v err=%v", response.Task, err)
	}
	if response.Task.AutomationID != automationID || response.Task.ProjectID != projectID || response.Task.ThreadName != "Automation continuation" {
		t.Fatalf("continuation provenance missing: %#v", response.Task)
	}
	if response.Task.Agent == nil || response.Task.Agent.Model != "gpt-5.4-mini" {
		t.Fatalf("continuation model override missing: %#v", response.Task.Agent)
	}
	if response.Task.AgentThreadMessage != "only follow this new direction" {
		t.Fatalf("continuation message = %q", response.Task.AgentThreadMessage)
	}
	if response.Task.AutomationDescription != "" {
		t.Fatalf("original Automation prompt leaked into continuation: %q", response.Task.AutomationDescription)
	}
	if response.Task.ID != continuationID {
		t.Fatalf("claimed task = %q, want continuation %q", response.Task.ID, continuationID)
	}
	if response.Task.AgentThreadRootTaskID != parentID {
		t.Fatalf("continuation root task = %q, want %q", response.Task.AgentThreadRootTaskID, parentID)
	}

	providerCheck := httptest.NewRecorder()
	providerRequest := withURLParams(
		newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/tasks/"+continuationID+"/provider-authorization", map[string]any{
			"provider": "codex", "model": "gpt-5.4-mini", "lease_id": "00000000-0000-0000-0000-000000000001",
		}, testWorkspaceID, "test-daemon"),
		"runtimeId", runtimeID, "taskId", continuationID,
	)
	testHandler.AuthorizeProviderOperation(providerCheck, providerRequest)
	if providerCheck.Code == http.StatusNotFound {
		t.Fatalf("provider authorization lost continuation workspace: %s", providerCheck.Body.String())
	}

	start := httptest.NewRecorder()
	startRequest := withURLParam(
		newDaemonTokenRequest("POST", "/api/daemon/tasks/"+continuationID+"/start", nil, testWorkspaceID, "test-daemon"),
		"taskId", continuationID,
	)
	testHandler.StartTask(start, startRequest)
	if start.Code != http.StatusOK {
		t.Fatalf("start continuation: status=%d body=%s", start.Code, start.Body.String())
	}

	report := httptest.NewRecorder()
	reportRequest := withURLParam(
		newDaemonTokenRequest("POST", "/api/daemon/tasks/"+continuationID+"/messages", map[string]any{
			"messages": []map[string]any{{"seq": 1, "type": "assistant", "content": "continuation evidence"}},
		}, testWorkspaceID, "test-daemon"),
		"taskId", continuationID,
	)
	testHandler.ReportTaskMessages(report, reportRequest)
	if report.Code != http.StatusOK {
		t.Fatalf("report continuation messages: status=%d body=%s", report.Code, report.Body.String())
	}

	list := httptest.NewRecorder()
	listRequest := withURLParam(newRequest("GET", "/api/tasks/"+continuationID+"/messages", nil), "taskId", continuationID)
	testHandler.ListTaskMessagesByUser(list, withChatTestWorkspaceCtx(t, listRequest))
	if list.Code != http.StatusOK || !strings.Contains(list.Body.String(), "continuation evidence") {
		t.Fatalf("list continuation messages: status=%d body=%s", list.Code, list.Body.String())
	}

	fail := httptest.NewRecorder()
	failRequest := withURLParam(
		newDaemonTokenRequest("POST", "/api/daemon/tasks/"+continuationID+"/fail", map[string]any{
			"error": "intentional lifecycle failure", "failure_reason": "agent_error.unknown",
		}, testWorkspaceID, "test-daemon"),
		"taskId", continuationID,
	)
	testHandler.FailTask(fail, failRequest)
	if fail.Code != http.StatusOK {
		t.Fatalf("fail continuation: status=%d body=%s", fail.Code, fail.Body.String())
	}
}

func TestAutomationAgentThreadContinuationClaimRechecksAutomationWrite(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Automation continuation revocation fixture", nil)
	var runtimeID string
	dbfx.QueryRow(t, `SELECT runtime_id FROM agent WHERE id = $1`, agentID).Scan(&runtimeID)
	automationID := dbfx.Insert(t, "automation", testutil.Cols{
		"workspace_id":    testWorkspaceID,
		"title":           "Automation continuation revocation",
		"model":           "gpt-5.4-mini",
		"executor_id":     agentID,
		"execution_mode":  "run_only",
		"created_by_type": "member",
		"created_by_id":   testUserID,
	})
	runID := dbfx.Insert(t, "automation_run", testutil.Cols{
		"automation_id": automationID,
		"source":        "manual",
		"status":        "completed",
	})
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":        runtimeID,
		"automation_run_id": runID,
		"status":            "completed",
		"session_id":        "automation-revocation-provider-thread",
		"completed_at":      testutil.Raw("now()"),
	})
	memberID := createPlainMember(t, "agent-thread-revoked-collaborator@example.com")
	dbfx.InsertNoID(t, "automation_collaborator", testutil.Cols{
		"automation_id": automationID,
		"user_type":     "member",
		"user_id":       memberID,
		"granted_by":    testUserID,
	}, "automation_id = $1 AND user_id = $2", automationID, memberID)
	member, err := testHandler.Queries.GetMemberByUserAndWorkspace(context.Background(), db.GetMemberByUserAndWorkspaceParams{
		UserID: util.MustParseUUID(memberID), WorkspaceID: util.MustParseUUID(testWorkspaceID),
	})
	if err != nil {
		t.Fatalf("load collaborator: %v", err)
	}

	continued := httptest.NewRecorder()
	continueRequest := newRequestAs(memberID, "POST", "/api/tasks/"+parentID+"/agent-thread/continue", map[string]any{"content": "run with the granted tools"})
	continueRequest = continueRequest.WithContext(middleware.SetMemberContext(continueRequest.Context(), testWorkspaceID, member))
	continueRequest = withURLParam(continueRequest, "taskId", parentID)
	continueRequest.Header.Set("Idempotency-Key", "automation-revocation-thread-1")
	testHandler.ContinueAgentThread(continued, continueRequest)
	if continued.Code != http.StatusOK {
		t.Fatalf("continue as collaborator: status=%d body=%s", continued.Code, continued.Body.String())
	}
	var receipt map[string]string
	if err := json.NewDecoder(continued.Body).Decode(&receipt); err != nil || receipt["continuation_task_id"] == "" {
		t.Fatalf("decode continuation receipt: %#v err=%v", receipt, err)
	}
	dbfx.Exec(t, `DELETE FROM automation_collaborator WHERE automation_id = $1 AND user_id = $2`, automationID, memberID)

	claim := httptest.NewRecorder()
	claimRequest := withURLParam(
		newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/claim", nil, testWorkspaceID, "test-daemon"),
		"runtimeId",
		runtimeID,
	)
	testHandler.ClaimTaskByRuntime(claim, claimRequest)
	if claim.Code != http.StatusForbidden {
		t.Fatalf("revoked continuation claim: status=%d body=%s", claim.Code, claim.Body.String())
	}
	var status, failureReason string
	dbfx.QueryRow(t, `SELECT status, failure_reason FROM agent_task_queue WHERE id = $1`, receipt["continuation_task_id"]).Scan(&status, &failureReason)
	if status != "failed" || failureReason != "invalid_task_identity" {
		t.Fatalf("revoked continuation settlement: status=%q reason=%q", status, failureReason)
	}
}

func TestAgentThreadSupportsOrdinaryQuickCreateTasks(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Quick-create Agent thread fixture", nil)
	runtimeID := handlerTestRuntimeID(t)
	var previousRuntimeMetadata string
	dbfx.QueryRow(t, `SELECT metadata FROM agent_runtime WHERE id = $1`, runtimeID).Scan(&previousRuntimeMetadata)
	dbfx.Exec(t, `UPDATE agent_runtime SET metadata = jsonb_build_object('cli_version', $2::text) WHERE id = $1`, runtimeID, agentpkg.MinQuickCreateCLIVersion)
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `UPDATE agent_runtime SET metadata = $2::jsonb WHERE id = $1`, runtimeID, previousRuntimeMetadata)
	})

	created := httptest.NewRecorder()
	createRequest := newRequest("POST", "/api/issues/quick-create", map[string]any{
		"agent_id": agentID,
		"prompt":   "Create the ordinary quick-create thread fixture",
	})
	testHandler.QuickCreateIssue(created, withChatTestWorkspaceCtx(t, createRequest))
	if created.Code != http.StatusAccepted {
		t.Fatalf("create ordinary quick-create task: status=%d body=%s", created.Code, created.Body.String())
	}
	var createResponse QuickCreateIssueResponse
	if err := json.NewDecoder(created.Body).Decode(&createResponse); err != nil || createResponse.TaskID == "" {
		t.Fatalf("decode ordinary quick-create response: %#v err=%v", createResponse, err)
	}
	taskID := createResponse.TaskID
	dbfx.Exec(t, `
		UPDATE agent_task_queue
		SET status = 'completed', session_id = 'quick-create-provider-thread-1', completed_at = now()
		WHERE id = $1
	`, taskID)
	var sourceContextID *string
	dbfx.QueryRow(t, `SELECT context->>'source_context_id' FROM agent_task_queue WHERE id = $1`, taskID).Scan(&sourceContextID)
	if sourceContextID != nil {
		t.Fatalf("ordinary quick-create unexpectedly has source_context_id %q", *sourceContextID)
	}

	get := httptest.NewRecorder()
	getRequest := withURLParam(newRequest("GET", "/api/tasks/"+taskID+"/agent-thread", nil), "taskId", taskID)
	testHandler.GetAgentThread(get, withChatTestWorkspaceCtx(t, getRequest))
	if get.Code != http.StatusOK {
		t.Fatalf("GET quick-create Agent thread: status=%d body=%s", get.Code, get.Body.String())
	}

	continued := httptest.NewRecorder()
	continueRequest := withURLParam(newRequest("POST", "/api/tasks/"+taskID+"/agent-thread/continue", map[string]any{"content": "finish creating this issue"}), "taskId", taskID)
	continueRequest.Header.Set("Idempotency-Key", "quick-create-thread-1")
	testHandler.ContinueAgentThread(continued, withChatTestWorkspaceCtx(t, continueRequest))
	if continued.Code != http.StatusOK {
		t.Fatalf("continue quick-create Agent thread: status=%d body=%s", continued.Code, continued.Body.String())
	}
}

func TestAgentThreadRejectsInvalidAndForeignQuickCreateRoots(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Invalid quick-create Agent thread fixture", nil)
	invalidTaskID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   handlerTestRuntimeID(t),
		"status":       "completed",
		"session_id":   "invalid-quick-create-provider-thread",
		"completed_at": testutil.Raw("now()"),
		"context": map[string]any{
			"type":         "forged_quick_create",
			"workspace_id": testWorkspaceID,
		},
	})

	assertNotFound := func(taskID string) {
		t.Helper()
		get := httptest.NewRecorder()
		request := withURLParam(newRequest("GET", "/api/tasks/"+taskID+"/agent-thread", nil), "taskId", taskID)
		testHandler.GetAgentThread(get, withChatTestWorkspaceCtx(t, request))
		if get.Code != http.StatusNotFound {
			t.Fatalf("invalid Agent thread root %s: status=%d body=%s", taskID, get.Code, get.Body.String())
		}
	}
	assertNotFound(invalidTaskID)

	foreignAgentID := createForeignWorkspaceAgent(t)
	var foreignWorkspaceID, foreignRuntimeID string
	dbfx.QueryRow(t, `SELECT workspace_id, runtime_id FROM agent WHERE id = $1`, foreignAgentID).Scan(&foreignWorkspaceID, &foreignRuntimeID)
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM workspace WHERE id = $1`, foreignWorkspaceID)
	})
	foreignTaskID := dbfx.Task(t, foreignAgentID, testutil.Cols{
		"runtime_id":   foreignRuntimeID,
		"status":       "completed",
		"session_id":   "foreign-quick-create-provider-thread",
		"completed_at": testutil.Raw("now()"),
		"context": map[string]any{
			"type":         "quick_create",
			"workspace_id": foreignWorkspaceID,
		},
	})
	assertNotFound(foreignTaskID)
}

func TestQuickCreateAgentThreadContinuationClaimWithoutIssueKeepsProjectAndWorkdir(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Quick-create continuation claim fixture", nil)
	var runtimeID string
	dbfx.QueryRow(t, `SELECT runtime_id FROM agent WHERE id = $1`, agentID).Scan(&runtimeID)
	projectID := dbfx.Project(t, "Quick-create continuation project", testutil.Cols{})
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   runtimeID,
		"status":       "completed",
		"session_id":   "quick-create-continuation-provider-thread",
		"work_dir":     "/workspace/quick-create-parent",
		"completed_at": testutil.Raw("now()"),
		"context": map[string]any{
			"type":              "quick_create",
			"prompt":            "ORIGINAL QUICK CREATE INSTRUCTIONS",
			"workspace_id":      testWorkspaceID,
			"project_id":        projectID,
			"source_context_id": "00000000-0000-0000-0000-000000000456",
		},
	})
	attachmentID := dbfx.Insert(t, "attachment", testutil.Cols{
		"workspace_id":  testWorkspaceID,
		"uploader_type": "member",
		"uploader_id":   testUserID,
		"filename":      "quick-create-follow-up.png",
		"url":           "https://cdn.example/quick-create-follow-up.png",
		"content_type":  "image/png",
		"size_bytes":    128,
	})
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM attachment WHERE id = $1`, attachmentID)
	})

	continued := httptest.NewRecorder()
	continueRequest := withURLParam(newRequest("POST", "/api/tasks/"+parentID+"/agent-thread/continue", map[string]any{
		"content":        "discuss the result instead",
		"attachment_ids": []string{attachmentID},
	}), "taskId", parentID)
	continueRequest.Header.Set("Idempotency-Key", "quick-create-claim-thread-1")
	testHandler.ContinueAgentThread(continued, withChatTestWorkspaceCtx(t, continueRequest))
	if continued.Code != http.StatusOK {
		t.Fatalf("continue quick-create Agent thread: status=%d body=%s", continued.Code, continued.Body.String())
	}
	var receipt map[string]string
	if err := json.NewDecoder(continued.Body).Decode(&receipt); err != nil || receipt["continuation_task_id"] == "" {
		t.Fatalf("decode quick-create continuation receipt: %#v err=%v", receipt, err)
	}
	child, err := testHandler.Queries.GetAgentTask(t.Context(), parseUUID(receipt["continuation_task_id"]))
	if err != nil || child.IssueID.Valid {
		t.Fatalf("quick-create continuation unexpectedly linked an issue: issue=%v err=%v", child.IssueID, err)
	}
	var childContext map[string]any
	if err := json.Unmarshal(child.Context, &childContext); err != nil {
		t.Fatalf("decode quick-create continuation context: %v", err)
	}
	if childContext["type"] != nil || childContext["workspace_id"] != nil || childContext["project_id"] != nil ||
		childContext["prompt"] != nil || childContext["source_context_id"] != nil {
		t.Fatalf("quick-create continuation routing context is unsafe or incomplete: %s", child.Context)
	}

	claim := httptest.NewRecorder()
	claimRequest := withURLParam(
		newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/claim", nil, testWorkspaceID, "test-daemon"),
		"runtimeId",
		runtimeID,
	)
	testHandler.ClaimTaskByRuntime(claim, claimRequest)
	if claim.Code != http.StatusOK {
		t.Fatalf("claim quick-create continuation: status=%d body=%s", claim.Code, claim.Body.String())
	}
	var response struct {
		Task *struct {
			WorkspaceID            string `json:"workspace_id"`
			ProjectID              string `json:"project_id"`
			PriorSessionID         string `json:"prior_session_id"`
			PriorWorkDir           string `json:"prior_work_dir"`
			AgentThreadMessage     string `json:"agent_thread_message"`
			QuickCreatePrompt      string `json:"quick_create_prompt"`
			AgentThreadAttachments []struct {
				ID          string `json:"id"`
				Filename    string `json:"filename"`
				ContentType string `json:"content_type"`
			} `json:"agent_thread_attachments"`
		} `json:"task"`
	}
	if err := json.NewDecoder(claim.Body).Decode(&response); err != nil || response.Task == nil {
		t.Fatalf("decode quick-create continuation claim: task=%#v err=%v", response.Task, err)
	}
	if response.Task.WorkspaceID != testWorkspaceID || response.Task.ProjectID != projectID {
		t.Fatalf("quick-create continuation scope missing: %#v", response.Task)
	}
	if response.Task.PriorSessionID != "quick-create-continuation-provider-thread" || response.Task.PriorWorkDir != "/workspace/quick-create-parent" {
		t.Fatalf("quick-create continuation resume context missing: %#v", response.Task)
	}
	if response.Task.AgentThreadMessage != "discuss the result instead" || response.Task.QuickCreatePrompt != "" {
		t.Fatalf("quick-create continuation prompt precedence wrong: %#v", response.Task)
	}
	if len(response.Task.AgentThreadAttachments) != 1 || response.Task.AgentThreadAttachments[0].ID != attachmentID ||
		response.Task.AgentThreadAttachments[0].Filename != "quick-create-follow-up.png" {
		t.Fatalf("quick-create continuation attachment context missing: %#v", response.Task.AgentThreadAttachments)
	}
}

func TestQuickCreateIssueLinkWaitsForContinuationAndIncludesIt(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Quick-create continuation link race fixture", nil)
	var runtimeID string
	dbfx.QueryRow(t, `SELECT runtime_id FROM agent WHERE id = $1`, agentID).Scan(&runtimeID)
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   runtimeID,
		"status":       "completed",
		"session_id":   "quick-create-link-race-thread",
		"completed_at": testutil.Raw("now()"),
		"context": map[string]any{
			"type":              "quick_create",
			"workspace_id":      testWorkspaceID,
			"source_context_id": "00000000-0000-0000-0000-000000000789",
		},
	})
	issueID := dbfx.Issue(t, "Quick-create link race result")

	writer, err := testPool.Begin(t.Context())
	if err != nil {
		t.Fatalf("begin continuation writer: %v", err)
	}
	defer writer.Rollback(context.Background())
	qtx := testHandler.Queries.WithTx(writer)
	if _, err := qtx.LockAgentThreadAgent(t.Context(), parseUUID(parentID)); err != nil {
		t.Fatalf("lock continuation Agent: %v", err)
	}
	holderPID := holderBackendPID(t, t.Context(), writer)
	if _, err := qtx.LockAgentThreadTask(t.Context(), parseUUID(parentID)); err != nil {
		t.Fatalf("lock continuation parent: %v", err)
	}
	firstChildID := dbid.NewV7()
	if _, err := qtx.CreateAgentThreadContinuation(t.Context(), db.CreateAgentThreadContinuationParams{
		ID:                   firstChildID,
		Content:              "first continuation while quick-create completes",
		IdempotencyKey:       "quick-create-link-race-first",
		RequesterUserID:      parseUUID(testUserID),
		RuntimeMcpOverlay:    []byte(`{}`),
		RuntimeConnectedApps: []byte(`[]`),
		ParentTaskID:         parseUUID(parentID),
	}); err != nil {
		t.Fatalf("insert first uncommitted continuation: %v", err)
	}
	secondChildID := dbid.NewV7()
	if _, err := qtx.CreateAgentThreadContinuation(t.Context(), db.CreateAgentThreadContinuationParams{
		ID:                   secondChildID,
		Content:              "second continuation while quick-create completes",
		IdempotencyKey:       "quick-create-link-race-second",
		RequesterUserID:      parseUUID(testUserID),
		RuntimeMcpOverlay:    []byte(`{}`),
		RuntimeConnectedApps: []byte(`[]`),
		ParentTaskID:         firstChildID,
	}); err != nil {
		t.Fatalf("insert second uncommitted continuation: %v", err)
	}
	if _, err := writer.Exec(t.Context(), `
		UPDATE agent_task_queue SET status = 'queued'
		WHERE id = ANY($1::uuid[])
	`, []pgtype.UUID{firstChildID, secondChildID}); err != nil {
		t.Fatalf("promote uncommitted continuations: %v", err)
	}

	linkDone := make(chan error, 1)
	go func() {
		linkDone <- testHandler.TaskService.LinkAgentThreadTaskToIssue(
			context.Background(), parseUUID(parentID), parseUUID(issueID),
		)
	}()
	if !waitForWaiterBlockedBy(t, holderPID, 5*time.Second) {
		t.Fatal("issue link did not wait for the in-flight continuation writer")
	}
	if err := writer.Commit(t.Context()); err != nil {
		t.Fatalf("commit continuation writer: %v", err)
	}
	select {
	case err := <-linkDone:
		if err != nil {
			t.Fatalf("link quick-create Agent thread: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("quick-create issue link did not finish after continuation committed")
	}

	for _, taskID := range []string{parentID, uuidToString(firstChildID), uuidToString(secondChildID)} {
		var linkedIssueID string
		dbfx.QueryRow(t, `SELECT issue_id FROM agent_task_queue WHERE id = $1`, taskID).Scan(&linkedIssueID)
		if linkedIssueID != issueID {
			t.Fatalf("task %s linked issue = %q, want %q", taskID, linkedIssueID, issueID)
		}
	}
	var queued, deferred int
	dbfx.QueryRow(t, `
		SELECT count(*) FILTER (WHERE status = 'queued'),
		       count(*) FILTER (WHERE status = 'deferred')
		FROM agent_task_queue
		WHERE id = ANY($1::uuid[])
	`, []pgtype.UUID{firstChildID, secondChildID}).Scan(&queued, &deferred)
	if queued != 1 || deferred != 1 {
		t.Fatalf("linked continuation pending states = queued:%d deferred:%d, want 1/1", queued, deferred)
	}
}

func TestQuickCreateIssueLinkDefersContinuationsBehindExistingPendingTask(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Quick-create existing pending slot fixture", nil)
	runtimeID := handlerTestRuntimeID(t)
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   runtimeID,
		"status":       "completed",
		"session_id":   "quick-create-existing-slot-thread",
		"completed_at": testutil.Raw("now()"),
		"context": map[string]any{
			"type":              "quick_create",
			"workspace_id":      testWorkspaceID,
			"source_context_id": "00000000-0000-0000-0000-000000000987",
		},
	})
	continueTask := func(taskID, content, key string) string {
		t.Helper()
		continued := httptest.NewRecorder()
		request := withURLParam(newRequest("POST", "/api/tasks/"+taskID+"/agent-thread/continue", map[string]any{"content": content}), "taskId", taskID)
		request.Header.Set("Idempotency-Key", key)
		testHandler.ContinueAgentThread(continued, withChatTestWorkspaceCtx(t, request))
		var receipt map[string]string
		if err := json.NewDecoder(continued.Body).Decode(&receipt); err != nil || continued.Code != http.StatusOK || receipt["continuation_task_id"] == "" {
			t.Fatalf("continue quick-create thread: status=%d receipt=%#v err=%v", continued.Code, receipt, err)
		}
		return receipt["continuation_task_id"]
	}
	firstChildID := continueTask(parentID, "first queued follow-up", "quick-create-existing-slot-first")
	secondChildID := continueTask(firstChildID, "second queued follow-up", "quick-create-existing-slot-second")
	dbfx.Exec(t, `UPDATE agent_task_queue SET status = 'queued' WHERE id = ANY($1::uuid[])`, []pgtype.UUID{parseUUID(firstChildID), parseUUID(secondChildID)})

	issueID := dbfx.Issue(t, "Quick-create result with existing pending task")
	existingID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id": runtimeID,
		"issue_id":   issueID,
		"status":     "queued",
	})
	if err := testHandler.TaskService.LinkAgentThreadTaskToIssue(t.Context(), parseUUID(parentID), parseUUID(issueID)); err != nil {
		t.Fatalf("link quick-create thread behind existing pending task: %v", err)
	}

	var existingStatus string
	dbfx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id = $1`, existingID).Scan(&existingStatus)
	if existingStatus != "queued" {
		t.Fatalf("existing issue pending task status = %q, want queued", existingStatus)
	}
	for _, taskID := range []string{firstChildID, secondChildID} {
		var linkedIssueID, status string
		dbfx.QueryRow(t, `SELECT issue_id, status FROM agent_task_queue WHERE id = $1`, taskID).Scan(&linkedIssueID, &status)
		if linkedIssueID != issueID || status != "deferred" {
			t.Fatalf("continuation %s = issue:%q status:%q, want issue:%q status:deferred", taskID, linkedIssueID, status, issueID)
		}
	}
}

func TestQuickCreateIssueLinkPreservesDispatchedContinuation(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Quick-create dispatched continuation fixture", nil)
	runtimeID := handlerTestRuntimeID(t)
	sessionID := "quick-create-dispatched-link-thread"
	parentID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":   runtimeID,
		"status":       "completed",
		"session_id":   sessionID,
		"completed_at": testutil.Raw("now()"),
		"context": map[string]any{
			"type":              "quick_create",
			"workspace_id":      testWorkspaceID,
			"source_context_id": "00000000-0000-0000-0000-000000000654",
		},
	})
	firstChildID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":               runtimeID,
		"status":                   "dispatched",
		"session_id":               sessionID,
		"dispatched_at":            testutil.Raw("now()"),
		"context":                  map[string]any{"agent_thread_parent_task_id": parentID, "agent_thread_message": "claimed follow-up"},
		"trigger_evidence_kind":    "agent_thread_continuation",
		"trigger_evidence_ref_id":  parentID,
		"prepare_lease_expires_at": testutil.Raw("now() + interval '1 minute'"),
	})
	secondChildID := dbfx.Task(t, agentID, testutil.Cols{
		"runtime_id":              runtimeID,
		"status":                  "queued",
		"session_id":              sessionID,
		"context":                 map[string]any{"agent_thread_parent_task_id": firstChildID, "agent_thread_message": "queued follow-up"},
		"trigger_evidence_kind":   "agent_thread_continuation",
		"trigger_evidence_ref_id": firstChildID,
	})
	issueID := dbfx.Issue(t, "Quick-create result with claimed continuation")
	if err := testHandler.TaskService.LinkAgentThreadTaskToIssue(t.Context(), parseUUID(parentID), parseUUID(issueID)); err != nil {
		t.Fatalf("link quick-create thread with claimed continuation: %v", err)
	}

	for taskID, wantStatus := range map[string]string{firstChildID: "dispatched", secondChildID: "deferred"} {
		var linkedIssueID, status string
		dbfx.QueryRow(t, `SELECT issue_id, status FROM agent_task_queue WHERE id = $1`, taskID).Scan(&linkedIssueID, &status)
		if linkedIssueID != issueID || status != wantStatus {
			t.Fatalf("continuation %s = issue:%q status:%q, want issue:%q status:%q", taskID, linkedIssueID, status, issueID, wantStatus)
		}
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
