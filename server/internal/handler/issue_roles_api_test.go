package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestCreateIssue_OwnerAndExecutorContract(t *testing.T) {
	if testHandler == nil {
		t.Skip("requires test database")
	}
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })

	agentID := dbfx.Agent(t, "roles-agent", testRuntimeID)

	create := func(body map[string]any) *httptest.ResponseRecorder {
		t.Helper()
		w := httptest.NewRecorder()
		testHandler.CreateIssue(w, newRequest("POST", "/api/issues?workspace_id="+testWorkspaceID, body))
		return w
	}

	t.Run("member owner is accepted", func(t *testing.T) {
		w := create(map[string]any{
			"title":      "owner contract",
			"status":     "todo",
			"owner_type": "member",
			"owner_id":   testUserID,
		})
		if w.Code != http.StatusCreated {
			t.Fatalf("create owner: expected 201, got %d: %s", w.Code, w.Body.String())
		}
		var issue IssueResponse
		if err := json.NewDecoder(w.Body).Decode(&issue); err != nil {
			t.Fatalf("decode: %v", err)
		}
		t.Cleanup(func() { deleteTestIssue(t, issue.ID) })
		if issue.OwnerType == nil || *issue.OwnerType != "member" || issue.OwnerID == nil || *issue.OwnerID != testUserID {
			t.Fatalf("owner fields: type=%v id=%v", issue.OwnerType, issue.OwnerID)
		}
		if issue.ExecutorType != nil || issue.ExecutorID != nil {
			t.Fatalf("executor should be empty, got type=%v id=%v", issue.ExecutorType, issue.ExecutorID)
		}
	})

	t.Run("agent executor is accepted", func(t *testing.T) {
		w := create(map[string]any{
			"title":         "executor contract",
			"status":        "todo",
			"executor_type": "agent",
			"executor_id":   agentID,
		})
		if w.Code != http.StatusCreated {
			t.Fatalf("create executor: expected 201, got %d: %s", w.Code, w.Body.String())
		}
		var issue IssueResponse
		if err := json.NewDecoder(w.Body).Decode(&issue); err != nil {
			t.Fatalf("decode: %v", err)
		}
		t.Cleanup(func() { deleteTestIssue(t, issue.ID) })
		if issue.ExecutorType == nil || *issue.ExecutorType != "agent" || issue.ExecutorID == nil || *issue.ExecutorID != agentID {
			t.Fatalf("executor fields: type=%v id=%v", issue.ExecutorType, issue.ExecutorID)
		}
	})

	t.Run("member as executor is rejected", func(t *testing.T) {
		w := create(map[string]any{
			"title":         "invalid executor",
			"status":        "todo",
			"executor_type": "member",
			"executor_id":   testUserID,
		})
		if w.Code != http.StatusBadRequest {
			t.Fatalf("expected 400, got %d: %s", w.Code, w.Body.String())
		}
	})

	t.Run("in_progress without executor is rejected", func(t *testing.T) {
		w := create(map[string]any{
			"title":  "active without executor",
			"status": "in_progress",
		})
		if w.Code != http.StatusBadRequest {
			t.Fatalf("expected 400, got %d: %s", w.Code, w.Body.String())
		}
	})
}

func TestUpdateIssue_ReviewHandoffRequiresDistinctReviewer(t *testing.T) {
	if testHandler == nil {
		t.Skip("requires test database")
	}
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })

	agentID := dbfx.Agent(t, "review-agent", testRuntimeID)
	create := httptest.NewRecorder()
	testHandler.CreateIssue(create, newRequest("POST", "/api/issues?workspace_id="+testWorkspaceID, map[string]any{
		"title":         "review handoff",
		"status":        "todo",
		"executor_type": "agent",
		"executor_id":   agentID,
	}))
	if create.Code != http.StatusCreated {
		t.Fatalf("create: expected 201, got %d: %s", create.Code, create.Body.String())
	}
	var issue IssueResponse
	if err := json.NewDecoder(create.Body).Decode(&issue); err != nil {
		t.Fatalf("decode: %v", err)
	}
	t.Cleanup(func() { deleteTestIssue(t, issue.ID) })

	w := httptest.NewRecorder()
	missing := newRequest("PUT", "/api/issues/"+issue.ID+"?workspace_id="+testWorkspaceID, map[string]any{
		"status": "in_review",
	})
	missing = withURLParam(missing, "id", issue.ID)
	testHandler.UpdateIssue(w, missing)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("in_review without reviewer: expected 400, got %d: %s", w.Code, w.Body.String())
	}

	ok := httptest.NewRecorder()
	withReviewer := newRequest("PUT", "/api/issues/"+issue.ID+"?workspace_id="+testWorkspaceID, map[string]any{
		"status":        "in_review",
		"reviewer_type": "member",
		"reviewer_id":   testUserID,
	})
	withReviewer = withURLParam(withReviewer, "id", issue.ID)
	testHandler.UpdateIssue(ok, withReviewer)
	if ok.Code != http.StatusOK {
		t.Fatalf("in_review with reviewer: expected 200, got %d: %s", ok.Code, ok.Body.String())
	}
}
