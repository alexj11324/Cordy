package handler

import (
	"context"
	"net/http"
	"testing"

	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

func requireIssueCategoryPolicyDatabase(t *testing.T) {
	t.Helper()
	if testHandler == nil || testPool == nil || dbfx == nil {
		t.Skip("database-backed handler test fixture is required")
	}
}

func issueCategoryPolicyRequest(userID, category string, body any) *http.Request {
	req := newRequestAs(userID, http.MethodPut, "/api/issue-category-policies/"+category, body)
	return testutil.WithURLParams(req, "category", category)
}

func policyErrorMessage(t *testing.T, response *testutil.Response) string {
	t.Helper()
	body := map[string]string{}
	response.JSON(&body)
	return body["error"]
}

func TestListIssueCategoryPoliciesIsWorkspaceScoped(t *testing.T) {
	requireIssueCategoryPolicyDatabase(t)

	runtimeID := dbfx.Runtime(t, "policy list runtime")
	agentID := dbfx.Agent(t, "policy list agent", runtimeID)
	_, err := testHandler.Queries.UpsertWorkspaceIssueCategoryPolicy(context.Background(), db.UpsertWorkspaceIssueCategoryPolicyParams{
		WorkspaceID:             parseUUID(testWorkspaceID),
		Category:                issuestatus.InProgress,
		DefaultExecutionAgentID: parseUUID(agentID),
	})
	if err != nil {
		t.Fatalf("seed workspace policy: %v", err)
	}
	dbfx.Cleanup(t, "DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1", testWorkspaceID)

	otherWorkspaceID := dbfx.Workspace(t, "policy list other workspace", "policy-list-other-"+uuid.NewString())
	_, err = testHandler.Queries.UpsertWorkspaceIssueCategoryPolicy(context.Background(), db.UpsertWorkspaceIssueCategoryPolicyParams{
		WorkspaceID: parseUUID(otherWorkspaceID),
		Category:    issuestatus.InReview,
	})
	if err != nil {
		t.Fatalf("seed other workspace policy: %v", err)
	}
	dbfx.Cleanup(t, "DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1", otherWorkspaceID)

	var response ListIssueCategoryPoliciesResponse
	testutil.Call(t, testHandler.ListIssueCategoryPolicies,
		newRequest(http.MethodGet, "/api/issue-category-policies", nil),
	).Want(http.StatusOK).JSON(&response)

	if len(response.Policies) != 1 {
		t.Fatalf("policy count = %d, want only the caller workspace policy", len(response.Policies))
	}
	if response.Policies[0].WorkspaceID != testWorkspaceID || response.Policies[0].Category != issuestatus.InProgress {
		t.Fatalf("unexpected workspace policy: %+v", response.Policies[0])
	}
}

func TestListIssueCategoryPoliciesRequiresWorkspaceMembership(t *testing.T) {
	requireIssueCategoryPolicyDatabase(t)
	outsiderID := dbfx.User(t, "policy outsider", "policy-outsider-"+uuid.NewString()+"@example.test")
	response := testutil.Call(t, testHandler.ListIssueCategoryPolicies,
		newRequestAs(outsiderID, http.MethodGet, "/api/issue-category-policies", nil),
	).Want(http.StatusNotFound)
	if got := policyErrorMessage(t, response); got != "workspace not found" {
		t.Fatalf("outsider error = %q, want workspace not found", got)
	}
}

func TestUpdateIssueCategoryPolicyMatchesRustValidationContract(t *testing.T) {
	requireIssueCategoryPolicyDatabase(t)
	runtimeID := dbfx.Runtime(t, "policy validation runtime")
	executionID := dbfx.Agent(t, "policy execution agent", runtimeID)
	noRuntimeID := dbfx.Agent(t, "policy no runtime agent", "")
	archivedID := dbfx.Agent(t, "policy archived agent", runtimeID, testutil.Cols{
		"archived_at": testutil.Raw("now()"),
	})

	otherWorkspaceID := dbfx.Workspace(t, "policy foreign workspace", "policy-foreign-"+uuid.NewString())
	foreignRuntimeID := dbfx.Runtime(t, "policy foreign runtime", testutil.Cols{"workspace_id": otherWorkspaceID})
	foreignAgentID := dbfx.Agent(t, "policy foreign agent", foreignRuntimeID, testutil.Cols{"workspace_id": otherWorkspaceID})

	memberID := dbfx.User(t, "policy member", "policy-member-"+uuid.NewString()+"@example.test")
	dbfx.Member(t, testWorkspaceID, memberID, "member")
	adminID := dbfx.User(t, "policy admin", "policy-admin-"+uuid.NewString()+"@example.test")
	dbfx.Member(t, testWorkspaceID, adminID, "admin")
	dbfx.Cleanup(t, "DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1", testWorkspaceID)

	validInProgress := map[string]any{"default_execution_agent_id": executionID}
	tests := []struct {
		name       string
		userID     string
		category   string
		body       any
		wantStatus int
		wantError  string
	}{
		{
			name:       "workspace member cannot update",
			userID:     memberID,
			category:   issuestatus.InProgress,
			body:       validInProgress,
			wantStatus: http.StatusForbidden,
			wantError:  "insufficient permissions",
		},
		{
			name:       "unsupported category",
			userID:     testUserID,
			category:   "done",
			body:       validInProgress,
			wantStatus: http.StatusBadRequest,
			wantError:  "unsupported issue category policy",
		},
		{
			name:       "invalid execution id",
			userID:     testUserID,
			category:   issuestatus.InProgress,
			body:       map[string]any{"default_execution_agent_id": "not-a-uuid"},
			wantStatus: http.StatusBadRequest,
			wantError:  "invalid default_execution_agent_id",
		},
		{
			name:       "invalid reviewer id",
			userID:     testUserID,
			category:   issuestatus.InProgress,
			body:       map[string]any{
				"default_execution_agent_id": executionID,
				"default_reviewer_agent_id":  "not-a-uuid",
			},
			wantStatus: http.StatusBadRequest,
			wantError:  "invalid default_reviewer_agent_id",
		},
		{
			name:       "execution is required",
			userID:     testUserID,
			category:   issuestatus.InProgress,
			body:       map[string]any{},
			wantStatus: http.StatusBadRequest,
			wantError:  "default_execution_agent_id is required",
		},
		{
			name:       "reviewer is required for in review",
			userID:     testUserID,
			category:   issuestatus.InReview,
			body:       validInProgress,
			wantStatus: http.StatusBadRequest,
			wantError:  "default_reviewer_agent_id is required for in_review",
		},
		{
			name:       "execution and reviewer must differ",
			userID:     testUserID,
			category:   issuestatus.InReview,
			body: map[string]any{
				"default_execution_agent_id": executionID,
				"default_reviewer_agent_id":  executionID,
			},
			wantStatus: http.StatusBadRequest,
			wantError:  "execution and review agents must differ",
		},
		{
			name:       "foreign workspace agent is unavailable",
			userID:     testUserID,
			category:   issuestatus.InProgress,
			body:       map[string]any{"default_execution_agent_id": foreignAgentID},
			wantStatus: http.StatusBadRequest,
			wantError:  "configured policy agent is unavailable",
		},
		{
			name:       "archived agent is unavailable",
			userID:     testUserID,
			category:   issuestatus.InProgress,
			body:       map[string]any{"default_execution_agent_id": archivedID},
			wantStatus: http.StatusBadRequest,
			wantError:  "configured policy agent is unavailable",
		},
		{
			name:       "agent without runtime is unavailable",
			userID:     testUserID,
			category:   issuestatus.InProgress,
			body:       map[string]any{"default_execution_agent_id": noRuntimeID},
			wantStatus: http.StatusBadRequest,
			wantError:  "configured policy agent is unavailable",
		},
		{
			name:       "admin may update",
			userID:     adminID,
			category:   issuestatus.InProgress,
			body:       validInProgress,
			wantStatus: http.StatusOK,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			response := testutil.Call(t, testHandler.UpdateIssueCategoryPolicy,
				issueCategoryPolicyRequest(tt.userID, tt.category, tt.body),
			).Want(tt.wantStatus)
			if tt.wantError != "" {
				got := policyErrorMessage(t, response)
				if got != tt.wantError {
					t.Fatalf("error = %q, want %q", got, tt.wantError)
				}
			}
		})
	}
}

func TestUpdateIssueCategoryPolicyRejectsMalformedJSON(t *testing.T) {
	requireIssueCategoryPolicyDatabase(t)
	req := testutil.JSONRequest(http.MethodPut, "/api/issue-category-policies/"+issuestatus.InProgress, "{")
	req.Header.Set("X-User-ID", testUserID)
	req.Header.Set("X-Workspace-ID", testWorkspaceID)
	req = testutil.WithURLParams(req, "category", issuestatus.InProgress)
	response := testutil.Call(t, testHandler.UpdateIssueCategoryPolicy, req).Want(http.StatusBadRequest)
	if got := policyErrorMessage(t, response); got != "invalid request body" {
		t.Fatalf("malformed body error = %q, want invalid request body", got)
	}
}

func TestUpdateIssueCategoryPolicyPersistsAndPublishesAfterCommit(t *testing.T) {
	requireIssueCategoryPolicyDatabase(t)
	runtimeID := dbfx.Runtime(t, "policy event runtime")
	executionID := dbfx.Agent(t, "policy event execution", runtimeID)
	reviewerID := dbfx.Agent(t, "policy event reviewer", runtimeID)

	changes := make(chan events.Event, 1)
	testHandler.Bus.Subscribe(protocol.EventIssueCategoryPolicyChanged, func(event events.Event) {
		changes <- event
	})

	var response IssueCategoryPolicyResponse
	testutil.Call(t, testHandler.UpdateIssueCategoryPolicy,
		issueCategoryPolicyRequest(testUserID, issuestatus.InReview, map[string]any{
			"default_execution_agent_id": executionID,
			"default_reviewer_agent_id":  reviewerID,
		}),
	).Want(http.StatusOK).JSON(&response)

	if response.WorkspaceID != testWorkspaceID || response.Category != issuestatus.InReview {
		t.Fatalf("unexpected policy response: %+v", response)
	}
	if response.DefaultExecutionAgentID == nil || *response.DefaultExecutionAgentID != executionID {
		t.Fatalf("execution agent response = %v, want %q", response.DefaultExecutionAgentID, executionID)
	}
	if response.DefaultReviewerAgentID == nil || *response.DefaultReviewerAgentID != reviewerID {
		t.Fatalf("reviewer agent response = %v, want %q", response.DefaultReviewerAgentID, reviewerID)
	}

	event := <-changes
	if event.WorkspaceID != testWorkspaceID || event.ActorType != "member" || event.ActorID != testUserID {
		t.Fatalf("policy event actor/scope = %s/%s/%s", event.WorkspaceID, event.ActorType, event.ActorID)
	}
	payload, ok := event.Payload.(map[string]any)
	if !ok || payload["category"] != issuestatus.InReview {
		t.Fatalf("policy event payload = %#v, want category %q", event.Payload, issuestatus.InReview)
	}

	stored, err := testHandler.Queries.GetWorkspaceIssueCategoryPolicy(context.Background(), db.GetWorkspaceIssueCategoryPolicyParams{
		WorkspaceID: parseUUID(testWorkspaceID),
		Category:    issuestatus.InReview,
	})
	if err != nil {
		t.Fatalf("read committed policy: %v", err)
	}
	if !stored.DefaultExecutionAgentID.Valid || uuidToString(stored.DefaultExecutionAgentID) != executionID {
		t.Fatalf("stored execution agent = %v, want %q", stored.DefaultExecutionAgentID, executionID)
	}
	if !stored.DefaultReviewerAgentID.Valid || uuidToString(stored.DefaultReviewerAgentID) != reviewerID {
		t.Fatalf("stored reviewer agent = %v, want %q", stored.DefaultReviewerAgentID, reviewerID)
	}
	dbfx.Cleanup(t, "DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1", testWorkspaceID)
}
