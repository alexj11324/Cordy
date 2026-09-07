package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func memoryRequest(t *testing.T, automationID, userID, method, name string, body any) *httptest.ResponseRecorder {
	t.Helper()
	path := "/api/automations/" + automationID + "/memories/" + name + "?workspace_id=" + testWorkspaceID
	if method == http.MethodDelete {
		path += "&revision=1"
	}
	r := newRequest(method, path, body)
	if userID != "" {
		r = newRequestAs(userID, method, path, body)
	}
	r = withURLParams(r, "id", automationID, "name", name)
	w := httptest.NewRecorder()
	switch method {
	case http.MethodPut:
		testHandler.PutAutomationMemory(w, r)
	case http.MethodDelete:
		testHandler.DeleteAutomationMemory(w, r)
	case http.MethodGet:
		if name == "" {
			testHandler.ListAutomationMemories(w, r)
		} else {
			testHandler.GetAutomationMemory(w, r)
		}
	}
	return w
}

func TestAutomationMemoryPersistenceAndConflict(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database required")
	}
	id := createAutomationAs(t, "", "memory-persistence")
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM automation_memory WHERE automation_id=$1`, id)
	})
	put := func(content string, revision int) *httptest.ResponseRecorder {
		return memoryRequest(t, id, "", http.MethodPut, "MEMORIES.md", map[string]any{"content": content, "expected_revision": revision})
	}
	if w := put("first run", 0); w.Code != 200 {
		t.Fatalf("create: %d %s", w.Code, w.Body.String())
	}
	if w := put("stale overwrite", 0); w.Code != 409 {
		t.Fatalf("stale write: %d %s", w.Code, w.Body.String())
	}
	w := memoryRequest(t, id, "", http.MethodGet, "MEMORIES.md", nil)
	var got struct {
		Content  string `json:"content"`
		Revision int    `json:"revision"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil || got.Content != "first run" || got.Revision != 1 {
		t.Fatalf("read: %s (%v)", w.Body.String(), err)
	}
	if w := memoryRequest(t, id, "", http.MethodDelete, "MEMORIES.md", nil); w.Code != 204 {
		t.Fatalf("delete: %d %s", w.Code, w.Body.String())
	}
	if w := memoryRequest(t, id, "", http.MethodGet, "MEMORIES.md", nil); w.Code != 404 {
		t.Fatalf("deleted read: %d", w.Code)
	}
	if w := put("recreated", 0); w.Code != 200 {
		t.Fatalf("recreate: %d %s", w.Code, w.Body.String())
	}
	if w := put("old editor", 1); w.Code != 409 {
		t.Fatalf("deleted revision reused: %d", w.Code)
	}
	if w := put("current edit", 3); w.Code != 200 {
		t.Fatalf("current write: %d %s", w.Code, w.Body.String())
	}
	if w := memoryRequest(t, id, "", http.MethodPut, "../escape.md", map[string]any{"content": "x", "expected_revision": 0}); w.Code != 400 {
		t.Fatalf("unsafe name: %d", w.Code)
	}
	if w := put(strings.Repeat("a", 65537), 4); w.Code != 400 {
		t.Fatalf("oversize: %d", w.Code)
	}
}

func TestAutomationMemoryRequiresWritePermission(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database required")
	}
	id := createAutomationAs(t, "", "memory-access")
	userID := createPlainMember(t, "automation-memory-reader@example.com")
	for _, method := range []string{http.MethodGet, http.MethodPut, http.MethodDelete} {
		w := memoryRequest(t, id, userID, method, "MEMORIES.md", map[string]any{"content": "x", "expected_revision": 0})
		if w.Code != 403 {
			t.Fatalf("%s: expected 403, got %d %s", method, w.Code, w.Body.String())
		}
	}
	if w := memoryRequest(t, id, userID, http.MethodGet, "", nil); w.Code != 403 {
		t.Fatalf("list: %d", w.Code)
	}
}

func TestAutomationMemoryTaskScopeAcrossRuns(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database required")
	}
	for _, mode := range []string{"run_only", "create_issue"} {
		t.Run(mode, func(t *testing.T) {
			id := createAutomationAs(t, "", "memory-task-"+mode)
			foreignID := createAutomationAs(t, "", "memory-foreign-"+mode)
			var agentID string
			dbfx.QueryRow(t, `UPDATE automation SET tools='{"memories":{"enabled":true}}', execution_mode=$2 WHERE id=$1 RETURNING executor_id`, id, mode).Scan(&agentID)
			dbfx.Exec(t, `UPDATE automation SET tools='{"memories":{"enabled":true}}' WHERE id=$1`, foreignID)
			t.Cleanup(func() {
				testPool.Exec(context.Background(), `DELETE FROM automation_memory WHERE automation_id=$1`, id)
			})
			cols := testutil.Cols{"status": "running", "runtime_id": handlerTestRuntimeID(t), "session_id": "memory-session-" + mode}
			if mode == "run_only" {
				cols["automation_run_id"] = dbfx.Insert(t, "automation_run", testutil.Cols{"automation_id": id, "source": "manual", "status": "running"})
			} else {
				cols["issue_id"] = dbfx.Issue(t, "memory origin", testutil.Cols{"origin_type": "automation", "origin_id": id})
			}
			taskID := dbfx.Task(t, agentID, cols)
			requestAsAgent := func(apID, requestAgentID, method string, body any) *httptest.ResponseRecorder {
				r := newRequest(method, "/api/automations/"+apID+"/memories/MEMORIES.md?workspace_id="+testWorkspaceID, body)
				r = withURLParams(r, "id", apID, "name", "MEMORIES.md")
				r.Header.Set("X-Actor-Source", "task_token")
				r.Header.Set("X-Agent-ID", requestAgentID)
				r.Header.Set("X-Task-ID", taskID)
				w := httptest.NewRecorder()
				if method == "PUT" {
					testHandler.PutAutomationMemory(w, r)
				} else {
					testHandler.GetAutomationMemory(w, r)
				}
				return w
			}
			request := func(apID, method string, body any) *httptest.ResponseRecorder {
				return requestAsAgent(apID, agentID, method, body)
			}
			if w := request(id, "PUT", map[string]any{"content": "run one finding", "expected_revision": 0}); w.Code != 200 {
				t.Fatalf("first task write: %d %s", w.Code, w.Body.String())
			}
			if w := request(foreignID, "GET", nil); w.Code != 403 {
				t.Fatalf("foreign automation: %d", w.Code)
			}
			dbfx.Exec(t, `UPDATE agent_task_queue SET status='completed' WHERE id=$1`, taskID)
			if w := request(id, "GET", nil); w.Code != 403 {
				t.Fatalf("completed task: %d", w.Code)
			}
			taskID = dbfx.Task(t, agentID, cols)
			if w := request(id, "GET", nil); w.Code != 200 || !strings.Contains(w.Body.String(), "run one finding") {
				t.Fatalf("next task read: %d %s", w.Code, w.Body.String())
			}
			for step := 0; step < 2; step++ {
				parentID := taskID
				dbfx.Exec(t, `UPDATE agent_task_queue SET status='completed' WHERE id=$1`, parentID)
				taskID = dbfx.Task(t, agentID, testutil.Cols{
					"status": "running", "runtime_id": cols["runtime_id"], "session_id": cols["session_id"],
					"issue_id":           cols["issue_id"],
					"originator_user_id": testUserID, "accountable_user_id": testUserID, "originator_source": "direct_human",
					"trigger_evidence_kind": "agent_thread_continuation", "trigger_evidence_ref_id": parentID,
					"context": map[string]any{"agent_thread_parent_task_id": parentID, "agent_thread_message": "read the memory"},
				})
				if w := request(id, "GET", nil); w.Code != 200 || !strings.Contains(w.Body.String(), "run one finding") {
					t.Fatalf("continuation %d read: %d %s", step, w.Code, w.Body.String())
				}
				if w := request(foreignID, "GET", nil); w.Code != 403 {
					t.Fatalf("continuation foreign automation: %d", w.Code)
				}
			}
			if mode == "run_only" {
				dbfx.Exec(t, `UPDATE agent_task_queue SET session_id='rotated-provider-session' WHERE id=$1`, taskID)
				if w := request(id, "GET", nil); w.Code != http.StatusOK {
					t.Fatalf("valid provider session rotation: %d %s", w.Code, w.Body.String())
				}

				var parentID, runtimeID string
				dbfx.QueryRow(t, `SELECT context->>'agent_thread_parent_task_id', runtime_id FROM agent_task_queue WHERE id=$1`, taskID).Scan(&parentID, &runtimeID)
				assertForgedContinuationDenied := func(name string, mutate, restore func(), requestAgentID string) {
					t.Helper()
					mutate()
					if w := requestAsAgent(id, requestAgentID, "GET", nil); w.Code != http.StatusForbidden {
						t.Fatalf("%s continuation lineage: %d %s", name, w.Code, w.Body.String())
					}
					restore()
				}
				assertForgedContinuationDenied("wrong parent",
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET context=jsonb_set(context, '{agent_thread_parent_task_id}', to_jsonb($2::text)) WHERE id=$1`, taskID, "00000000-0000-0000-0000-000000000123")
					},
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET context=jsonb_set(context, '{agent_thread_parent_task_id}', to_jsonb($2::text)) WHERE id=$1`, taskID, parentID)
					}, agentID)
				assertForgedContinuationDenied("wrong evidence",
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET trigger_evidence_ref_id=$2 WHERE id=$1`, taskID, "00000000-0000-0000-0000-000000000124")
					},
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET trigger_evidence_ref_id=$2 WHERE id=$1`, taskID, parentID)
					}, agentID)

				foreignRunID := dbfx.Insert(t, "automation_run", testutil.Cols{"automation_id": foreignID, "source": "manual", "status": "running"})
				foreignRootID := dbfx.Task(t, agentID, testutil.Cols{
					"status": "completed", "runtime_id": runtimeID, "session_id": "foreign-root-session",
					"automation_run_id": foreignRunID, "completed_at": testutil.Raw("now()"),
				})
				assertForgedContinuationDenied("wrong root",
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET context=jsonb_set(context, '{agent_thread_parent_task_id}', to_jsonb($2::text)), trigger_evidence_ref_id=$2::uuid WHERE id=$1`, taskID, foreignRootID)
					},
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET context=jsonb_set(context, '{agent_thread_parent_task_id}', to_jsonb($2::text)), trigger_evidence_ref_id=$2::uuid WHERE id=$1`, taskID, parentID)
					}, agentID)

				foreignRuntimeID := dbfx.Insert(t, "agent_runtime", testutil.Cols{
					"workspace_id": testWorkspaceID, "daemon_id": nil, "name": "Memory forged runtime",
					"runtime_mode": "cloud", "provider": "memory_forged_runtime", "status": "online",
					"device_info": "Memory forged runtime", "metadata": testutil.Raw("'{}'::jsonb"), "last_seen_at": testutil.Raw("now()"),
				})
				assertForgedContinuationDenied("wrong runtime",
					func() {
						dbfx.Exec(t, `UPDATE agent_task_queue SET runtime_id=$2 WHERE id=$1`, taskID, foreignRuntimeID)
					},
					func() { dbfx.Exec(t, `UPDATE agent_task_queue SET runtime_id=$2 WHERE id=$1`, taskID, runtimeID) }, agentID)

				foreignAgentID := createHandlerTestAgent(t, "Memory forged continuation agent", nil)
				assertForgedContinuationDenied("wrong agent",
					func() { dbfx.Exec(t, `UPDATE agent_task_queue SET agent_id=$2 WHERE id=$1`, taskID, foreignAgentID) },
					func() { dbfx.Exec(t, `UPDATE agent_task_queue SET agent_id=$2 WHERE id=$1`, taskID, agentID) }, foreignAgentID)
			}
			outsiderID := createPlainMember(t, "memory-continuation-outsider-"+mode+"@test.local")
			dbfx.Exec(t, `UPDATE agent_task_queue SET originator_user_id=$2, accountable_user_id=$2 WHERE id=$1`, taskID, outsiderID)
			if w := request(id, "GET", nil); w.Code != 403 {
				t.Fatalf("noncollaborator continuation read: %d", w.Code)
			}
			if w := request(id, "PUT", map[string]any{"content": "unauthorized overwrite", "expected_revision": 1}); w.Code != 403 {
				t.Fatalf("noncollaborator continuation write: %d", w.Code)
			}
			grantAutomationAccess(t, "", id, outsiderID, http.StatusCreated)
			if w := request(id, "GET", nil); w.Code != 200 {
				t.Fatalf("granted continuation read: %d %s", w.Code, w.Body.String())
			}
			dbfx.Exec(t, `DELETE FROM automation_collaborator WHERE automation_id=$1 AND user_id=$2`, id, outsiderID)
			if w := request(id, "GET", nil); w.Code != 403 {
				t.Fatalf("revoked continuation read: %d", w.Code)
			}
			dbfx.Exec(t, `UPDATE agent_task_queue SET originator_user_id=$2, accountable_user_id=$2 WHERE id=$1`, taskID, testUserID)
			dbfx.Exec(t, `UPDATE automation SET tools='{"memories":{"enabled":false}}' WHERE id=$1`, id)
			if w := request(id, "GET", nil); w.Code != 200 {
				t.Fatalf("legacy disabled memory row should remain in use: %d", w.Code)
			}
		})
	}
}
