package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestAutomationAssigneeSaveUsesAutomationOwnerInvocation(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	privateAgentID, ownerID, _ := privateAgentTestFixture(t)
	publicAgentID := createHandlerTestAgent(t, "automation-assignee-public", nil)

	create := func(t *testing.T, creatorID string) AutomationResponse {
		t.Helper()
		w := httptest.NewRecorder()
		req := newRequestAs(creatorID, http.MethodPost, "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
			"title":          "assignee invocation save " + creatorID,
			"executor_id":    publicAgentID,
			"execution_mode": "run_only",
		})
		testHandler.CreateAutomation(w, req)
		if w.Code != http.StatusCreated {
			t.Fatalf("CreateAutomation: expected 201, got %d: %s", w.Code, w.Body.String())
		}
		var response AutomationResponse
		if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
			t.Fatalf("decode automation: %v", err)
		}
		t.Cleanup(func() {
			_, _ = testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, response.ID)
		})
		return response
	}

	patchExecutor := func(t *testing.T, automationID, actorID string) *httptest.ResponseRecorder {
		t.Helper()
		w := httptest.NewRecorder()
		req := newRequestAs(actorID, http.MethodPatch, "/api/automations/"+automationID+"?workspace_id="+testWorkspaceID, map[string]any{
			"executor_type": "agent",
			"executor_id":   privateAgentID,
		})
		req = withURLParam(req, "id", automationID)
		testHandler.UpdateAutomation(w, req)
		return w
	}

	t.Run("foreign private agent is rejected on create", func(t *testing.T) {
		const title = "foreign private create must fail"
		w := httptest.NewRecorder()
		req := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
			"title":          title,
			"executor_id":    privateAgentID,
			"execution_mode": "run_only",
		})
		testHandler.CreateAutomation(w, req)
		if w.Code != http.StatusForbidden {
			t.Fatalf("foreign private executor create = %d, want 403: %s", w.Code, w.Body.String())
		}
		var count int
		if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM automation WHERE title = $1`, title).Scan(&count); err != nil {
			t.Fatalf("count denied automation: %v", err)
		}
		if count != 0 {
			t.Fatalf("denied create persisted %d automation rows", count)
		}
	})

	t.Run("foreign private agent is rejected and old executor remains", func(t *testing.T) {
		automation := create(t, testUserID)
		response := patchExecutor(t, automation.ID, testUserID)
		if response.Code != http.StatusForbidden {
			t.Fatalf("foreign private executor update = %d, want 403: %s", response.Code, response.Body.String())
		}
		var executorID string
		if err := testPool.QueryRow(context.Background(), `SELECT executor_id::text FROM automation WHERE id = $1`, automation.ID).Scan(&executorID); err != nil {
			t.Fatalf("read executor after denied update: %v", err)
		}
		if executorID != publicAgentID {
			t.Fatalf("denied update changed executor to %s, want %s", executorID, publicAgentID)
		}
	})

	t.Run("private agent owner can save the executor", func(t *testing.T) {
		automation := create(t, ownerID)
		response := patchExecutor(t, automation.ID, ownerID)
		if response.Code != http.StatusOK {
			t.Fatalf("owner private executor update = %d, want 200: %s", response.Code, response.Body.String())
		}
		var executorID string
		if err := testPool.QueryRow(context.Background(), `SELECT executor_id::text FROM automation WHERE id = $1`, automation.ID).Scan(&executorID); err != nil {
			t.Fatalf("read executor after allowed update: %v", err)
		}
		if executorID != privateAgentID {
			t.Fatalf("allowed update executor = %s, want %s", executorID, privateAgentID)
		}
	})
}
