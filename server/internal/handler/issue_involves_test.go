package handler

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// involvesFixture seeds, for a single test, the data needed to exercise every
// branch of the `involves_user_id` 4-branch filter — owned agent, team human
// member, team canonical leader (via team.leader_id, NOT a team_member copy
// row), and team agent member — plus a parallel set in a second workspace so
// the cross-workspace negative tests can prove subquery-level isolation.
type involvesFixture struct {
	// All IDs are in the primary handler-test workspace unless noted otherwise.
	userID  string // the "me" user the filter is keyed on (== testUserID)
	otherID string // a different user in the same workspace

	ownedAgentID   string // agent.owner_id = userID — branch (1) seed
	otherAgentID   string // agent.owner_id = otherID — must NOT match
	teamMemberID   string // team with userID as human member — branch (2)
	teamLeaderID   string // team whose leader_id is an agent owned by userID — branch (3)
	teamAgentMemID string // team with an owned-agent as team_member row — branch (4)

	// Other workspace, mirror objects — used by ExcludesOtherWorkspace* tests
	otherWsID           string
	otherWsAgent        string // owned by userID but in other workspace
	otherWsTeamMember   string // team with userID as human member, in other ws
	otherWsTeamLeader   string // team whose leader is userID's agent (in other ws)
	otherWsTeamAgentMem string // team with userID's agent as member (in other ws)
}

func setupInvolvesFixture(t *testing.T) *involvesFixture {
	t.Helper()
	ctx := context.Background()
	suffix := time.Now().UnixNano()

	fx := &involvesFixture{userID: testUserID}

	// --- second user inside the primary workspace ---
	var otherUserID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO "user" (name, email) VALUES ($1, $2) RETURNING id
	`, "Involves Other User", fmt.Sprintf("involves-other-%d@patchbay.ai", suffix)).Scan(&otherUserID); err != nil {
		t.Fatalf("create other user: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, otherUserID) })
	fx.otherID = otherUserID
	if _, err := testPool.Exec(ctx, `
		INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'member')
	`, testWorkspaceID, otherUserID); err != nil {
		t.Fatalf("create other member: %v", err)
	}

	runtimeID := handlerTestRuntimeID(t)

	// --- agents in primary workspace ---
	fx.ownedAgentID = insertAgent(t, ctx, testWorkspaceID, runtimeID, fx.userID,
		fmt.Sprintf("Involves Owned Agent %d", suffix))
	fx.otherAgentID = insertAgent(t, ctx, testWorkspaceID, runtimeID, fx.otherID,
		fmt.Sprintf("Involves Other Agent %d", suffix))

	// --- a team we already have to satisfy NOT NULL leader_id ---
	leaderForMemberTeam := insertAgent(t, ctx, testWorkspaceID, runtimeID, fx.otherID,
		fmt.Sprintf("Involves Leader-for-MemberTeam %d", suffix))
	fx.teamMemberID = insertTeam(t, ctx, testWorkspaceID, leaderForMemberTeam,
		fmt.Sprintf("InvolvesTeamMember-%d", suffix))
	// Add the test user as a human member.
	if _, err := testPool.Exec(ctx, `
		INSERT INTO team_member (team_id, member_type, member_id) VALUES ($1, 'member', $2)
	`, fx.teamMemberID, fx.userID); err != nil {
		t.Fatalf("add team human member: %v", err)
	}

	// --- team with leader = our owned agent — branch (3). Critically, we do
	// NOT insert a team_member row for the leader, so the test exercises the
	// canonical team.leader_id path. ---
	fx.teamLeaderID = insertTeam(t, ctx, testWorkspaceID, fx.ownedAgentID,
		fmt.Sprintf("InvolvesTeamLeader-%d", suffix))

	// --- team whose agent member is our owned agent — branch (4) ---
	leaderForAgentMemTeam := insertAgent(t, ctx, testWorkspaceID, runtimeID, fx.otherID,
		fmt.Sprintf("Involves Leader-for-AgentMemTeam %d", suffix))
	fx.teamAgentMemID = insertTeam(t, ctx, testWorkspaceID, leaderForAgentMemTeam,
		fmt.Sprintf("InvolvesTeamAgentMem-%d", suffix))
	// Use a fresh owned agent so the team_member row is the only signal —
	// keeps branch (4) independent from branch (1)/(3).
	branch4Agent := insertAgent(t, ctx, testWorkspaceID, runtimeID, fx.userID,
		fmt.Sprintf("Involves Branch4 Agent %d", suffix))
	if _, err := testPool.Exec(ctx, `
		INSERT INTO team_member (team_id, member_type, member_id) VALUES ($1, 'agent', $2)
	`, fx.teamAgentMemID, branch4Agent); err != nil {
		t.Fatalf("add team agent member: %v", err)
	}

	// --- second workspace, mirroring all four shapes for cross-ws negatives ---
	var otherWsID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO workspace (name, slug, description, issue_prefix)
		VALUES ($1, $2, '', 'OTH')
		RETURNING id
	`, fmt.Sprintf("InvolvesOtherWs-%d", suffix), fmt.Sprintf("involves-other-ws-%d", suffix)).Scan(&otherWsID); err != nil {
		t.Fatalf("create other workspace: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM workspace WHERE id = $1`, otherWsID) })
	fx.otherWsID = otherWsID

	// Membership in other workspace (so the user could legitimately be an executor
	// there too — exercises whether subquery workspace_id clause filters it out).
	if _, err := testPool.Exec(ctx, `
		INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'owner')
	`, otherWsID, fx.userID); err != nil {
		t.Fatalf("create other-ws member: %v", err)
	}

	var otherRuntimeID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_runtime (
			workspace_id, daemon_id, name, runtime_mode, provider, status, device_info, metadata, last_seen_at
		) VALUES ($1, NULL, $2, 'cloud', 'other_ws_runtime', 'online', $3, '{}'::jsonb, now())
		RETURNING id
	`, otherWsID, fmt.Sprintf("OtherWs Runtime %d", suffix), "other-ws-runtime").Scan(&otherRuntimeID); err != nil {
		t.Fatalf("create other-ws runtime: %v", err)
	}

	fx.otherWsAgent = insertAgent(t, ctx, otherWsID, otherRuntimeID, fx.userID,
		fmt.Sprintf("OtherWs Owned Agent %d", suffix))

	leaderForOtherWsMemberTeam := insertAgent(t, ctx, otherWsID, otherRuntimeID, fx.otherID,
		fmt.Sprintf("OtherWs Leader-for-MemberTeam %d", suffix))
	fx.otherWsTeamMember = insertTeam(t, ctx, otherWsID, leaderForOtherWsMemberTeam,
		fmt.Sprintf("OtherWsTeamMember-%d", suffix))
	if _, err := testPool.Exec(ctx, `
		INSERT INTO team_member (team_id, member_type, member_id) VALUES ($1, 'member', $2)
	`, fx.otherWsTeamMember, fx.userID); err != nil {
		t.Fatalf("add other-ws team member: %v", err)
	}

	fx.otherWsTeamLeader = insertTeam(t, ctx, otherWsID, fx.otherWsAgent,
		fmt.Sprintf("OtherWsTeamLeader-%d", suffix))

	leaderForOtherWsAgentMemTeam := insertAgent(t, ctx, otherWsID, otherRuntimeID, fx.otherID,
		fmt.Sprintf("OtherWs Leader-for-AgentMemTeam %d", suffix))
	fx.otherWsTeamAgentMem = insertTeam(t, ctx, otherWsID, leaderForOtherWsAgentMemTeam,
		fmt.Sprintf("OtherWsTeamAgentMem-%d", suffix))
	otherWsBranch4Agent := insertAgent(t, ctx, otherWsID, otherRuntimeID, fx.userID,
		fmt.Sprintf("OtherWs Branch4 Agent %d", suffix))
	if _, err := testPool.Exec(ctx, `
		INSERT INTO team_member (team_id, member_type, member_id) VALUES ($1, 'agent', $2)
	`, fx.otherWsTeamAgentMem, otherWsBranch4Agent); err != nil {
		t.Fatalf("add other-ws team agent member: %v", err)
	}

	return fx
}

func insertAgent(t *testing.T, ctx context.Context, workspaceID, runtimeID, ownerID, name string) string {
	t.Helper()
	var id string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent (
			workspace_id, name, description, runtime_mode, runtime_config,
			runtime_id, visibility, max_concurrent_tasks, owner_id
		)
		VALUES ($1, $2, '', 'cloud', '{}'::jsonb, $3, 'workspace', 1, $4)
		RETURNING id
	`, workspaceID, name, runtimeID, ownerID).Scan(&id); err != nil {
		t.Fatalf("create agent %q: %v", name, err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM agent WHERE id = $1`, id) })
	return id
}

func insertTeam(t *testing.T, ctx context.Context, workspaceID, leaderAgentID, name string) string {
	t.Helper()
	var id string
	// creator_id is loose (no FK) — reuse testUserID to keep the row valid.
	if err := testPool.QueryRow(ctx, `
		INSERT INTO team (workspace_id, name, leader_id, creator_id)
		VALUES ($1, $2, $3, $4)
		RETURNING id
	`, workspaceID, name, leaderAgentID, testUserID).Scan(&id); err != nil {
		t.Fatalf("create team %q: %v", name, err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM team WHERE id = $1`, id) })
	return id
}

// insertIssueTo creates an issue in the given workspace with the given role
// (roleType, roleID) pair and returns its UUID. Issue rows are
// best-effort-cleaned up by the test.
func insertIssueTo(t *testing.T, ctx context.Context, workspaceID, title, roleType, roleID string) string {
	t.Helper()
	var number int32
	if err := testPool.QueryRow(ctx, `
		UPDATE workspace
		SET issue_counter = GREATEST(
			issue_counter,
			(SELECT COALESCE(MAX(number), 0) FROM issue WHERE workspace_id = $1)
		) + 1
		WHERE id = $1
		RETURNING issue_counter
	`, workspaceID).Scan(&number); err != nil {
		t.Fatalf("next issue number: %v", err)
	}
	var id string
	var ownerType, executorType any
	var ownerID, executorID any
	if roleType == "member" {
		ownerType, ownerID = roleType, roleID
	} else {
		executorType, executorID = roleType, roleID
	}
	if err := testPool.QueryRow(ctx, `
		INSERT INTO issue (
			workspace_id, title, description, status, priority,
			owner_type, owner_id, executor_type, executor_id, creator_type, creator_id,
			position, number
		)
		VALUES ($1, $2, NULL, 'todo', 'none', $3, $4, $5, $6, 'member', $7, 0, $8)
		RETURNING id
	`, workspaceID, title, ownerType, ownerID, executorType, executorID, testUserID, number).Scan(&id); err != nil {
		t.Fatalf("create issue %q: %v", title, err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM issue WHERE id = $1`, id) })
	return id
}

// listIssuesInvolves runs ListIssues with `involves_user_id` set to userID
// (against testWorkspaceID) and returns the resulting issue IDs.
func listIssuesInvolves(t *testing.T, userID string) []string {
	t.Helper()
	path := fmt.Sprintf("/api/issues?workspace_id=%s&involves_user_id=%s&limit=500",
		testWorkspaceID, userID)
	w := httptest.NewRecorder()
	testHandler.ListIssues(w, newRequest("GET", path, nil))
	if w.Code != http.StatusOK {
		t.Fatalf("ListIssues: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var resp struct {
		Issues []IssueResponse `json:"issues"`
	}
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode list response: %v", err)
	}
	ids := make([]string, 0, len(resp.Issues))
	for _, iss := range resp.Issues {
		ids = append(ids, iss.ID)
	}
	return ids
}

// listGroupedIssuesInvolves runs ListGroupedIssues with `involves_user_id`
// set to userID and returns the flattened set of issue IDs across all groups.
func listGroupedIssuesInvolves(t *testing.T, userID string) []string {
	t.Helper()
	path := fmt.Sprintf(
		"/api/issues/grouped?workspace_id=%s&group_by=executor&statuses=todo&involves_user_id=%s&limit=100",
		testWorkspaceID, userID)
	w := httptest.NewRecorder()
	testHandler.ListGroupedIssues(w, newRequest("GET", path, nil))
	if w.Code != http.StatusOK {
		t.Fatalf("ListGroupedIssues: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var resp GroupedIssuesResponse
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode grouped response: %v", err)
	}
	ids := []string{}
	for _, g := range resp.Groups {
		for _, iss := range g.Issues {
			ids = append(ids, iss.ID)
		}
	}
	return ids
}

func containsIssueID(ids []string, target string) bool {
	for _, id := range ids {
		if id == target {
			return true
		}
	}
	return false
}

// ---- positive branches ----

func TestListIssues_InvolvesUserID_MatchesOwnedAgentExecutor(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	wantID := insertIssueTo(t, ctx, testWorkspaceID,
		"issue executed by my owned agent", "agent", fx.ownedAgentID)
	if got := listIssuesInvolves(t, fx.userID); !containsIssueID(got, wantID) {
		t.Fatalf("branch (1) miss: owned-agent executor not surfaced (want %s, got %v)", wantID, got)
	}
}

func TestListIssues_InvolvesUserID_MatchesTeamMember(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	wantID := insertIssueTo(t, ctx, testWorkspaceID,
		"issue executed by a team I'm a member of", "team", fx.teamMemberID)
	if got := listIssuesInvolves(t, fx.userID); !containsIssueID(got, wantID) {
		t.Fatalf("branch (2) miss: human-member team executor not surfaced (want %s, got %v)", wantID, got)
	}
}

func TestListIssues_InvolvesUserID_MatchesLeaderViaCanonicalRelation(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	// Fixture deliberately omits the team_member leader-copy row, so this
	// can only match if the SQL reads team.leader_id directly (branch 3).
	wantID := insertIssueTo(t, ctx, testWorkspaceID,
		"issue executed by a team my agent leads", "team", fx.teamLeaderID)
	if got := listIssuesInvolves(t, fx.userID); !containsIssueID(got, wantID) {
		t.Fatalf("branch (3) miss: team-leader-via-canonical executor not surfaced (want %s, got %v)", wantID, got)
	}
}

func TestListIssues_InvolvesUserID_MatchesTeamAgentMember(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	wantID := insertIssueTo(t, ctx, testWorkspaceID,
		"issue executed by a team my agent is a member of", "team", fx.teamAgentMemID)
	if got := listIssuesInvolves(t, fx.userID); !containsIssueID(got, wantID) {
		t.Fatalf("branch (4) miss: team agent-member executor not surfaced (want %s, got %v)", wantID, got)
	}
}

// ---- the critical negative: tab 3 must be disjoint from tab 1 ----

// Nails the semantics: `involves_user_id` MUST NOT surface issues whose
// owner is the user themself (member type). Direct member ownership is
// the meaning of `owner_id` (tab 1 "Owned by me"); the two tabs must
// produce disjoint result sets. If anyone adds a fifth UNION branch
// `(executor_type='member' AND executor_id=involves_user_id)` back in, this
// test fails.
func TestListIssues_InvolvesUserID_ExcludesDirectMemberOwner(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	issueID := insertIssueTo(t, ctx, testWorkspaceID,
		"tab 3 must NOT surface member-direct ownership", "member", fx.userID)
	if got := listIssuesInvolves(t, fx.userID); containsIssueID(got, issueID) {
		t.Fatalf("tab 3 semantics violated: involves_user_id surfaced a member-owned issue (id=%s); that belongs to tab 1. Full result: %v",
			issueID, got)
	}
}

// Same negative on the grouped (dynamic SQL) path — the dynamic builder is a
// separate code path from sqlc, so it gets its own regression.
func TestListGroupedIssues_InvolvesUserID_ExcludesDirectMemberOwner(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	issueID := insertIssueTo(t, ctx, testWorkspaceID,
		"grouped tab 3 must NOT surface member-direct ownership", "member", fx.userID)
	if got := listGroupedIssuesInvolves(t, fx.userID); containsIssueID(got, issueID) {
		t.Fatalf("grouped tab 3 semantics violated: involves_user_id surfaced a member-owned issue (id=%s); full result: %v",
			issueID, got)
	}
}

// ---- workspace isolation negatives — each subquery must clamp workspace_id ----

func TestListIssues_InvolvesUserID_ExcludesOtherWorkspaceAgent(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	// Issue lives in the *primary* workspace but is executed by an agent UUID
	// that only exists in the OTHER workspace and is owned by our user. If the
	// agent subquery is missing `a.workspace_id = $1`, this match would leak.
	issueID := insertIssueTo(t, ctx, testWorkspaceID,
		"cross-ws agent executor must not leak", "agent", fx.otherWsAgent)
	if got := listIssuesInvolves(t, fx.userID); containsIssueID(got, issueID) {
		t.Fatalf("workspace isolation violated: cross-workspace agent surfaced (id=%s); full result: %v",
			issueID, got)
	}
}

func TestListIssues_InvolvesUserID_ExcludesOtherWorkspaceLeader(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	issueID := insertIssueTo(t, ctx, testWorkspaceID,
		"cross-ws team-leader executor must not leak", "team", fx.otherWsTeamLeader)
	if got := listIssuesInvolves(t, fx.userID); containsIssueID(got, issueID) {
		t.Fatalf("workspace isolation violated: cross-workspace team-leader surfaced (id=%s); full result: %v",
			issueID, got)
	}
}

func TestListIssues_InvolvesUserID_ExcludesOtherWorkspaceTeamMember(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	issueID := insertIssueTo(t, ctx, testWorkspaceID,
		"cross-ws team-human-member executor must not leak", "team", fx.otherWsTeamMember)
	if got := listIssuesInvolves(t, fx.userID); containsIssueID(got, issueID) {
		t.Fatalf("workspace isolation violated: cross-workspace team-human-member surfaced (id=%s); full result: %v",
			issueID, got)
	}
}

func TestListIssues_InvolvesUserID_ExcludesOtherWorkspaceTeamAgentMember(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	issueID := insertIssueTo(t, ctx, testWorkspaceID,
		"cross-ws team-agent-member executor must not leak", "team", fx.otherWsTeamAgentMem)
	if got := listIssuesInvolves(t, fx.userID); containsIssueID(got, issueID) {
		t.Fatalf("workspace isolation violated: cross-workspace team-agent-member surfaced (id=%s); full result: %v",
			issueID, got)
	}
}

// ---- combo + boundary ----

// involves_user_id and creator_id must AND together — combining narrowing
// filters should never widen the result.
func TestListIssues_InvolvesUserID_CombinesWithCreatorID(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	// Issue with creator = otherID: involves matches (branch 1) but creator
	// filter must exclude it.
	exclude := insertIssueTo(t, ctx, testWorkspaceID,
		"involves matches but creator does not", "agent", fx.ownedAgentID)
	// Patch the creator to otherID directly.
	if _, err := testPool.Exec(ctx, `UPDATE issue SET creator_id = $1 WHERE id = $2`, fx.otherID, exclude); err != nil {
		t.Fatalf("patch creator: %v", err)
	}

	path := fmt.Sprintf("/api/issues?workspace_id=%s&involves_user_id=%s&creator_id=%s&limit=500",
		testWorkspaceID, fx.userID, fx.userID)
	w := httptest.NewRecorder()
	testHandler.ListIssues(w, newRequest("GET", path, nil))
	if w.Code != http.StatusOK {
		t.Fatalf("ListIssues: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	var resp struct {
		Issues []IssueResponse `json:"issues"`
	}
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode list response: %v", err)
	}
	got := make([]string, 0, len(resp.Issues))
	for _, iss := range resp.Issues {
		got = append(got, iss.ID)
	}
	if containsIssueID(got, exclude) {
		t.Fatalf("combined filter widened result: issue %s with non-matching creator surfaced; full result: %v",
			exclude, got)
	}
}

func TestListIssues_InvolvesUserID_InvalidUUIDReturns400(t *testing.T) {
	path := fmt.Sprintf("/api/issues?workspace_id=%s&involves_user_id=not-a-uuid", testWorkspaceID)
	w := httptest.NewRecorder()
	testHandler.ListIssues(w, newRequest("GET", path, nil))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 on invalid UUID, got %d: %s", w.Code, w.Body.String())
	}
}

// Grouped path also exercises canonical-leader resolution, so a single positive
// guards the dynamic SQL builder against accidentally dropping branch (3).
func TestListGroupedIssues_InvolvesUserID_MatchesLeaderViaCanonicalRelation(t *testing.T) {
	ctx := context.Background()
	fx := setupInvolvesFixture(t)
	wantID := insertIssueTo(t, ctx, testWorkspaceID,
		"grouped: team my agent leads", "team", fx.teamLeaderID)
	if got := listGroupedIssuesInvolves(t, fx.userID); !containsIssueID(got, wantID) {
		t.Fatalf("grouped branch (3) miss: team-leader-via-canonical not surfaced (want %s, got %v)", wantID, got)
	}
}
