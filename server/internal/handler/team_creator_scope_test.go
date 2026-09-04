package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/go-chi/chi/v5"
)

// teamScopeReq builds a request as the given user (empty = workspace owner)
// with the chi URL params the team handlers read (workspaceId + optional id).
// The team handlers resolve the workspace from workspaceIDFromURL, which reads
// the chi route context, not the query string — so tests must inject the params
// here rather than on the path.
func teamScopeReq(userID, method, path string, body any, params map[string]string) *http.Request {
	var req *http.Request
	if userID == "" {
		req = newRequest(method, path, body)
	} else {
		req = newRequestAs(userID, method, path, body)
	}
	rctx := chi.NewRouteContext()
	rctx.URLParams.Add("workspaceId", testWorkspaceID)
	for k, v := range params {
		rctx.URLParams.Add(k, v)
	}
	return req.WithContext(context.WithValue(req.Context(), chi.RouteCtxKey, rctx))
}

// createTeamAs creates a team through the handler as the given user and
// returns the decoded response. Registers cleanup for the team + its members.
func createTeamAs(t *testing.T, userID, name, leaderID string) TeamResponse {
	t.Helper()
	w := httptest.NewRecorder()
	r := teamScopeReq(userID, "POST", "/api/teams", map[string]any{
		"name":      name,
		"leader_id": leaderID,
	}, nil)
	testHandler.CreateTeam(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("CreateTeam(%s): expected 201, got %d: %s", name, w.Code, w.Body.String())
	}
	var resp TeamResponse
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode team: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM team_member WHERE team_id = $1`, resp.ID)
		testPool.Exec(context.Background(), `DELETE FROM team WHERE id = $1`, resp.ID)
	})
	return resp
}

// TestCreateTeam_PlainMemberBecomesCreator verifies the gate change: a plain
// workspace member (not owner/admin) can create a team and is recorded as its
// creator.
func TestCreateTeam_PlainMemberBecomesCreator(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	memberID := createPlainMember(t, "team-creator@patchbay.test")
	leaderID := createHandlerTestAgent(t, "team-creator-leader", nil)

	team := createTeamAs(t, memberID, "Member Owned Team", leaderID)
	if team.CreatorID != memberID {
		t.Fatalf("expected creator_id=%s, got %s", memberID, team.CreatorID)
	}
}

// TestManageTeam_CreatorCanManageOwn verifies a creator can update, add a
// member to, and archive their own team.
func TestManageTeam_CreatorCanManageOwn(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	memberID := createPlainMember(t, "team-owner-manage@patchbay.test")
	leaderID := createHandlerTestAgent(t, "team-owner-manage-leader", nil)
	worker := createHandlerTestAgent(t, "team-owner-manage-worker", nil)

	team := createTeamAs(t, memberID, "Manage Own Team", leaderID)

	// Update name.
	w := httptest.NewRecorder()
	testHandler.UpdateTeam(w, teamScopeReq(memberID, "PATCH", "/api/teams", map[string]any{
		"name": "Renamed By Creator",
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusOK {
		t.Fatalf("UpdateTeam as creator: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	// Add a public agent worker.
	w = httptest.NewRecorder()
	testHandler.AddTeamMember(w, teamScopeReq(memberID, "POST", "/api/teams/members", map[string]any{
		"member_type": "agent",
		"member_id":   worker,
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusCreated {
		t.Fatalf("AddTeamMember as creator: expected 201, got %d: %s", w.Code, w.Body.String())
	}

	// Archive.
	w = httptest.NewRecorder()
	testHandler.DeleteTeam(w, teamScopeReq(memberID, "DELETE", "/api/teams", nil,
		map[string]string{"id": team.ID}))
	if w.Code != http.StatusNoContent {
		t.Fatalf("DeleteTeam as creator: expected 204, got %d: %s", w.Code, w.Body.String())
	}
}

// TestManageTeam_StrangerMemberForbidden verifies a plain member who did not
// create the team cannot manage it, while a workspace admin/owner still can.
func TestManageTeam_StrangerMemberForbidden(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	creatorID := createPlainMember(t, "team-stranger-creator@patchbay.test")
	strangerID := createPlainMember(t, "team-stranger-other@patchbay.test")
	leaderID := createHandlerTestAgent(t, "team-stranger-leader", nil)

	team := createTeamAs(t, creatorID, "Stranger Test Team", leaderID)

	// Stranger member: update denied.
	w := httptest.NewRecorder()
	testHandler.UpdateTeam(w, teamScopeReq(strangerID, "PATCH", "/api/teams", map[string]any{
		"name": "Hijacked",
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusForbidden {
		t.Fatalf("UpdateTeam as stranger: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	// Stranger member: archive denied.
	w = httptest.NewRecorder()
	testHandler.DeleteTeam(w, teamScopeReq(strangerID, "DELETE", "/api/teams", nil,
		map[string]string{"id": team.ID}))
	if w.Code != http.StatusForbidden {
		t.Fatalf("DeleteTeam as stranger: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	// Workspace owner (testUserID): update allowed — admin management unchanged.
	w = httptest.NewRecorder()
	testHandler.UpdateTeam(w, teamScopeReq("", "PATCH", "/api/teams", map[string]any{
		"name": "Renamed By Admin",
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusOK {
		t.Fatalf("UpdateTeam as workspace owner: expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

// TestAddTeamMember_CreatorAgentAccessGate verifies the comment-#2 rule: a
// non-admin creator may add a public agent (invocable) but not a private agent
// they cannot @-trigger. The workspace owner may add the same private agent —
// admin wiring is unrestricted.
func TestAddTeamMember_CreatorAgentAccessGate(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	privateAgentID, _, memberID := privateAgentTestFixture(t)
	publicLeaderID := createHandlerTestAgent(t, "team-gate-leader", nil)
	publicWorkerID := createHandlerTestAgent(t, "team-gate-worker", nil)

	team := createTeamAs(t, memberID, "Agent Gate Team", publicLeaderID)

	// Creator adds a public (invocable) worker — allowed.
	w := httptest.NewRecorder()
	testHandler.AddTeamMember(w, teamScopeReq(memberID, "POST", "/api/teams/members", map[string]any{
		"member_type": "agent",
		"member_id":   publicWorkerID,
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusCreated {
		t.Fatalf("AddTeamMember public agent: expected 201, got %d: %s", w.Code, w.Body.String())
	}

	// Creator adds a private agent they cannot invoke — denied.
	w = httptest.NewRecorder()
	testHandler.AddTeamMember(w, teamScopeReq(memberID, "POST", "/api/teams/members", map[string]any{
		"member_type": "agent",
		"member_id":   privateAgentID,
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusForbidden {
		t.Fatalf("AddTeamMember private agent as creator: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	// Workspace owner adds the same private agent — allowed (admin unchanged).
	w = httptest.NewRecorder()
	testHandler.AddTeamMember(w, teamScopeReq("", "POST", "/api/teams/members", map[string]any{
		"member_type": "agent",
		"member_id":   privateAgentID,
	}, map[string]string{"id": team.ID}))
	if w.Code != http.StatusCreated {
		t.Fatalf("AddTeamMember private agent as owner: expected 201, got %d: %s", w.Code, w.Body.String())
	}
}

// TestCreateTeam_CreatorPrivateLeaderForbidden verifies a non-admin cannot
// create a team led by a private agent they cannot @-trigger.
func TestCreateTeam_CreatorPrivateLeaderForbidden(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	privateAgentID, _, memberID := privateAgentTestFixture(t)

	w := httptest.NewRecorder()
	r := teamScopeReq(memberID, "POST", "/api/teams", map[string]any{
		"name":      "Private Leader Team",
		"leader_id": privateAgentID,
	}, nil)
	testHandler.CreateTeam(w, r)
	if w.Code != http.StatusForbidden {
		// Nothing should have been created; if it slipped through, clean up.
		if w.Code == http.StatusCreated {
			var resp TeamResponse
			if json.NewDecoder(w.Body).Decode(&resp) == nil {
				testPool.Exec(context.Background(), `DELETE FROM team_member WHERE team_id = $1`, resp.ID)
				testPool.Exec(context.Background(), `DELETE FROM team WHERE id = $1`, resp.ID)
			}
		}
		t.Fatalf("CreateTeam with private leader: expected 403, got %d: %s", w.Code, w.Body.String())
	}
}
