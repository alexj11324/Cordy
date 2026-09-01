package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// TestTeamOperatingProtocolRecordsAgainstTheTurnsIssue locks the recording
// contract the protocol teaches (MUL-6622 / GH #7487): record against the issue
// this turn runs on — team assignment of that issue is irrelevant — and, when
// the call fails, leave a trace WITHOUT breaking one-comment-per-turn. The
// conditional matters: on the `action` path a delegation comment already exists,
// so an unconditional "post a comment on failure" would demand a second one.
func TestTeamOperatingProtocolRecordsAgainstTheTurnsIssue(t *testing.T) {
	for _, ownsStatus := range []bool{true, false} {
		compact := strings.Join(strings.Fields(teamOperatingProtocolFor(ownsStatus)), " ")
		for _, want := range []string{
			"Record it against the issue THIS turn is running on",
			"It does not need to be assigned to your team",
			"without breaking the one-comment-per-turn rule",
			"ONLY if you have not already commented this turn",
			"do not add a second one",
			"never post a second comment just to report the error",
		} {
			if !strings.Contains(compact, want) {
				t.Errorf("owns_status=%v: expected team operating protocol to contain %q\n--- protocol ---\n%s",
					ownsStatus, want, compact)
			}
		}
	}
}

// TestTeamOperatingProtocolOwnsParentStatus locks the parent-issue status
// contract: first dispatch moves todo→in_progress and stays there; only a
// later confirmation of overall completion may advance to in_review; done is
// left to humans / integrations.
func TestTeamOperatingProtocolOwnsParentStatus(t *testing.T) {
	protocol := teamOperatingProtocolFor(true)
	compact := strings.Join(strings.Fields(protocol), " ")
	for _, want := range []string{
		"Own the parent issue status",
		"move the parent to `in_progress`",
		"successful dispatch is not completion",
		"patchbay issue status <issue-id> in_review",
		"Leave `done` to a human reviewer",
	} {
		if !strings.Contains(compact, want) {
			t.Errorf("expected team operating protocol to contain %q\n--- protocol ---\n%s", want, protocol)
		}
	}
}

// TestTeamOperatingProtocolScopesParentStatusOwnership is the guard for the
// MUL-5156 review finding: the briefing is injected on every leader path,
// including an @team mention on an issue assigned to someone else. Status
// ownership must not ride along — a guest leader gets an explicit prohibition
// instead of the grant, so the model never has to infer the boundary.
func TestTeamOperatingProtocolScopesParentStatusOwnership(t *testing.T) {
	guest := teamOperatingProtocolFor(false)
	compactGuest := strings.Join(strings.Fields(guest), " ")

	for _, want := range []string{
		"Do NOT change this issue's status",
		"not assigned to your team",
		"never run `patchbay issue status` on it",
	} {
		if !strings.Contains(compactGuest, want) {
			t.Errorf("expected guest-leader protocol to contain %q\n--- protocol ---\n%s", want, guest)
		}
	}
	// The grant must be entirely absent — not merely qualified.
	for _, forbidden := range []string{
		"Own the parent issue status",
		"patchbay issue status <issue-id> in_review",
	} {
		if strings.Contains(compactGuest, forbidden) {
			t.Errorf("guest-leader protocol must not contain status grant %q\n--- protocol ---\n%s", forbidden, guest)
		}
	}

	// Everything that is not the status responsibility is identical, so a
	// guest leader still coordinates, delegates, and records activity.
	owner := teamOperatingProtocolFor(true)
	for _, shared := range []string{
		"## Team Operating Protocol",
		"Delegate by @mention",
		"Record your evaluation",
		"Stop after dispatching",
		"Never both for the same work.",
	} {
		if !strings.Contains(owner, shared) || !strings.Contains(guest, shared) {
			t.Errorf("expected %q in both protocol variants", shared)
		}
	}

	// Both variants must keep the protocol header. The daemon no longer
	// derives IsTeamLeader from it (MUL-5811 — it reads is_leader_task /
	// team_id off the claim), but it is still the section title the leader
	// rules in the brief and the per-turn prompt refer to by name.
	if !strings.Contains(guest, "## Team Operating Protocol") {
		t.Error("guest-leader protocol lost its section header")
	}
}

// TestTeamOperatingProtocolWarnsAgainstDualTrigger locks in the rule
// added for #3033: the protocol must tell the team leader that a `todo`
// child issue with an agent assignee already fires that agent, so they
// must not also @mention the same agent on the parent issue for the
// same work. Asserts behavior, not exact wording — keep the substrings
// narrow so harmless rewording doesn't break the test.
func TestTeamOperatingProtocolWarnsAgainstDualTrigger(t *testing.T) {
	protocol := teamOperatingProtocolFor(true)
	compact := strings.Join(strings.Fields(protocol), " ")
	for _, want := range []string{
		"--status todo` and an agent assignee already fires that agent automatically",
		"Never both for the same work.",
	} {
		if !strings.Contains(compact, want) {
			t.Errorf("expected team operating protocol to contain %q\n--- protocol ---\n%s", want, protocol)
		}
	}
}

// seedTeamForBriefing creates a team with the seeded test agent as
// leader. Returns the loaded db.Team and a cleanup-registered ID.
func seedTeamForBriefing(t *testing.T, leaderID string, name, instructions string) db.Team {
	t.Helper()
	ctx := context.Background()

	var teamID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO team (workspace_id, name, description, leader_id, creator_id, instructions)
		VALUES ($1, $2, '', $3, $4, $5)
		RETURNING id
	`, testWorkspaceID, name, leaderID, testUserID, instructions).Scan(&teamID); err != nil {
		t.Fatalf("create team: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(ctx, `DELETE FROM team WHERE id = $1`, teamID)
	})

	uuid := util.MustParseUUID(teamID)
	team, err := testHandler.Queries.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{
		ID:          uuid,
		WorkspaceID: util.MustParseUUID(testWorkspaceID),
	})
	if err != nil {
		t.Fatalf("load team: %v", err)
	}
	return team
}

func addAgentMember(t *testing.T, teamID pgtype.UUID, agentID, role string) {
	t.Helper()
	if _, err := testHandler.Queries.AddTeamMember(context.Background(), db.AddTeamMemberParams{
		TeamID:    teamID,
		MemberType: "agent",
		MemberID:   util.MustParseUUID(agentID),
		Role:       role,
	}); err != nil {
		t.Fatalf("add agent member: %v", err)
	}
}

func addHumanMember(t *testing.T, teamID pgtype.UUID, userID, role string) {
	t.Helper()
	if _, err := testHandler.Queries.AddTeamMember(context.Background(), db.AddTeamMemberParams{
		TeamID:    teamID,
		MemberType: "member",
		MemberID:   util.MustParseUUID(userID),
		Role:       role,
	}); err != nil {
		t.Fatalf("add human member: %v", err)
	}
}

// seededLeaderAgent loads the first seeded agent in the test workspace.
func seededLeaderAgent(t *testing.T) (id, name string) {
	t.Helper()
	if err := testPool.QueryRow(context.Background(), `
		SELECT id, name FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1
	`, testWorkspaceID).Scan(&id, &name); err != nil {
		t.Fatalf("load seeded agent: %v", err)
	}
	return id, name
}

// seededHumanMember returns the (member_row_id, user_id, user_name) of the
// test fixture's human member in the workspace.
func seededHumanMember(t *testing.T) (memberID, userID, userName string) {
	t.Helper()
	if err := testPool.QueryRow(context.Background(), `
		SELECT m.id, u.id, u.name
		FROM member m JOIN "user" u ON u.id = m.user_id
		WHERE m.workspace_id = $1 ORDER BY m.created_at ASC LIMIT 1
	`, testWorkspaceID).Scan(&memberID, &userID, &userName); err != nil {
		t.Fatalf("load seeded member: %v", err)
	}
	return
}

func TestBuildTeamLeaderBriefing_FullTeam(t *testing.T) {
	ctx := context.Background()
	leaderID, leaderName := seededLeaderAgent(t)

	team := seedTeamForBriefing(t, leaderID, "Full Team", "Always write tests.")

	helper1 := createHandlerTestAgent(t, "Helper One", []byte("[]"))
	helper2 := createHandlerTestAgent(t, "Helper Two", []byte("[]"))
	addAgentMember(t, team.ID, helper1, "implementer")
	addAgentMember(t, team.ID, helper2, "")

	memberRowID, userID, userName := seededHumanMember(t)
	_ = memberRowID
	addHumanMember(t, team.ID, userID, "reviewer")

	out := buildTeamLeaderBriefing(ctx, testHandler.Queries, team, true)

	for _, want := range []string{
		"## Team Operating Protocol",
		"## Team Roster",
		"Leader (you):",
		leaderName,
		"## Team Instructions (Full Team)",
		"Always write tests.",
		"`[@Helper One](mention://agent/" + helper1 + ")`",
		"`[@Helper Two](mention://agent/" + helper2 + ")`",
		`role: "implementer"`,
		`role: "reviewer"`,
		"`[@" + userName + "](mention://member/" + userID + ")`",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("expected briefing to contain %q\n--- briefing ---\n%s", want, out)
		}
	}

	// Helper Two has no role — must NOT render an empty role: "" segment.
	if strings.Contains(out, `Helper Two — agent, role: ""`) {
		t.Errorf("expected empty role to be omitted, got: %s", out)
	}
}

// assignSkillToAgent creates a workspace skill and attaches it to the agent.
func assignSkillToAgent(t *testing.T, agentID, skillName string) {
	t.Helper()
	ctx := context.Background()
	var skillID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO skill (workspace_id, name, description, content, created_by)
		VALUES ($1, $2, '', '', $3)
		RETURNING id
	`, testWorkspaceID, skillName, testUserID).Scan(&skillID); err != nil {
		t.Fatalf("create skill %s: %v", skillName, err)
	}
	t.Cleanup(func() {
		if _, err := testPool.Exec(ctx, `DELETE FROM agent_skill WHERE agent_id = $1 AND skill_id = $2`, agentID, skillID); err != nil {
			t.Errorf("cleanup agent skill %s/%s: %v", agentID, skillName, err)
		}
		if _, err := testPool.Exec(ctx, `DELETE FROM skill WHERE id = $1`, skillID); err != nil {
			t.Errorf("cleanup skill %s: %v", skillName, err)
		}
	})
	if _, err := testPool.Exec(ctx,
		`INSERT INTO agent_skill (agent_id, skill_id) VALUES ($1, $2)`,
		agentID, skillID,
	); err != nil {
		t.Fatalf("assign skill %s to agent: %v", skillName, err)
	}
}

// TestBuildTeamLeaderBriefing_MemberSkillsInRoster locks in the delegation
// fix: an agent member's assigned skills appear in the leader roster so the
// leader can route by capability. Agents with no skills get an explicit
// marker; human members never carry a skills segment.
func TestBuildTeamLeaderBriefing_MemberSkillsInRoster(t *testing.T) {
	ctx := context.Background()
	leaderID, _ := seededLeaderAgent(t)
	team := seedTeamForBriefing(t, leaderID, "Skilled Team", "")

	skilled := createHandlerTestAgent(t, "Skilled Bot", []byte("[]"))
	addAgentMember(t, team.ID, skilled, "backend")
	// ListAgentSkillNamesByAgentIDs orders by name ASC → "polars" before "stat…".
	assignSkillToAgent(t, skilled, "polars")
	assignSkillToAgent(t, skilled, "statistical-analysis")

	plain := createHandlerTestAgent(t, "Plain Bot", []byte("[]"))
	addAgentMember(t, team.ID, plain, "")

	memberRowID, userID, userName := seededHumanMember(t)
	_ = memberRowID
	addHumanMember(t, team.ID, userID, "reviewer")

	out := buildTeamLeaderBriefing(ctx, testHandler.Queries, team, true)

	if !strings.Contains(out, "skills: polars, statistical-analysis") {
		t.Errorf("expected skilled member skills in roster, got:\n%s", out)
	}
	if !strings.Contains(out, "Plain Bot — agent — no skills assigned") {
		t.Errorf("expected no-skills marker for skill-less agent, got:\n%s", out)
	}
	if strings.Contains(out, userName+" — member (human), role: \"reviewer\" — skills:") ||
		strings.Contains(out, userName+" — member (human), role: \"reviewer\" — no skills") {
		t.Errorf("human member must not render a skills segment, got:\n%s", out)
	}
}

func TestBuildTeamLeaderBriefing_OnlyLeader(t *testing.T) {
	ctx := context.Background()
	leaderID, _ := seededLeaderAgent(t)
	team := seedTeamForBriefing(t, leaderID, "Solo Team", "")

	out := buildTeamLeaderBriefing(ctx, testHandler.Queries, team, true)
	if !strings.Contains(out, "Members: (none — you are the only member of this team)") {
		t.Errorf("expected lone-leader fallback line, got:\n%s", out)
	}
	// No user instructions → no Team Instructions section.
	if strings.Contains(out, "## Team Instructions") {
		t.Errorf("expected no Team Instructions section when empty, got:\n%s", out)
	}
}

func TestBuildTeamLeaderBriefing_SkipsArchivedAgent(t *testing.T) {
	ctx := context.Background()
	leaderID, _ := seededLeaderAgent(t)
	team := seedTeamForBriefing(t, leaderID, "Archive Team", "")

	archived := createHandlerTestAgent(t, "Retired Bot", []byte("[]"))
	addAgentMember(t, team.ID, archived, "")
	if _, err := testPool.Exec(ctx,
		`UPDATE agent SET archived_at = now(), archived_by = $1 WHERE id = $2`,
		testUserID, archived,
	); err != nil {
		t.Fatalf("archive agent: %v", err)
	}

	out := buildTeamLeaderBriefing(ctx, testHandler.Queries, team, true)
	if strings.Contains(out, "Retired Bot") {
		t.Errorf("archived agent should not appear in roster:\n%s", out)
	}
	if strings.Contains(out, archived) {
		t.Errorf("archived agent UUID should not appear in roster:\n%s", out)
	}
}

// TestBuildTeamLeaderBriefing_MentionsRoundTrip is the contract test
// guaranteeing every emitted mention markdown string parses back through
// util.ParseMentions to its (type, id). If this ever breaks, the leader's
// dispatch comments will silently fail to trigger anyone.
func TestBuildTeamLeaderBriefing_MentionsRoundTrip(t *testing.T) {
	ctx := context.Background()
	leaderID, _ := seededLeaderAgent(t)
	team := seedTeamForBriefing(t, leaderID, "Mention Round Trip", "")

	helper := createHandlerTestAgent(t, "Round Trip Bot", []byte("[]"))
	addAgentMember(t, team.ID, helper, "")

	memberRowID, userID, _ := seededHumanMember(t)
	_ = memberRowID
	addHumanMember(t, team.ID, userID, "")

	out := buildTeamLeaderBriefing(ctx, testHandler.Queries, team, true)
	mentions := util.ParseMentions(out)

	wantIDs := map[string]string{
		leaderID: "agent",
		helper:   "agent",
		userID:   "member",
	}
	got := make(map[string]string, len(mentions))
	for _, m := range mentions {
		got[m.ID] = m.Type
	}
	for id, kind := range wantIDs {
		if got[id] != kind {
			t.Errorf("expected %s mention for id %s, got %q (all parsed: %#v)", kind, id, got[id], mentions)
		}
	}
}

// claimAndDecodeAgent runs ClaimTaskByRuntime for the given runtime and
// returns the agent block of the response. Fails the test on non-200.
func claimAndDecodeAgent(t *testing.T, runtimeID string) *TaskAgentData {
	t.Helper()
	w := httptest.NewRecorder()
	req := newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/claim", nil, testWorkspaceID, "test-claim-team-briefing")
	req = withURLParam(req, "runtimeId", runtimeID)
	testHandler.ClaimTaskByRuntime(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("ClaimTaskByRuntime: %d %s", w.Code, w.Body.String())
	}
	var resp struct {
		Task *struct {
			Agent *TaskAgentData `json:"agent"`
		} `json:"task"`
	}
	if err := json.NewDecoder(w.Body).Decode(&resp); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if resp.Task == nil || resp.Task.Agent == nil {
		t.Fatalf("expected task.agent in response, got: %s", w.Body.String())
	}
	return resp.Task.Agent
}

// queueTeamIssueTaskFor creates an issue assigned to the team and a queued
// task for the given (agentID, runtimeID). Returns the issue + task IDs.
func queueTeamIssueTaskFor(t *testing.T, teamID, agentID, runtimeID string, issueNumber int) (issueID, taskID string) {
	t.Helper()
	ctx := context.Background()
	if err := testPool.QueryRow(ctx, `
INSERT INTO issue (
workspace_id, title, status, priority, creator_id, creator_type,
executor_type, executor_id, number, position
) VALUES ($1, 'Team briefing claim test', 'todo', 'medium', $2, 'member',
'team', $3, $4, 0)
RETURNING id
`, testWorkspaceID, testUserID, teamID, issueNumber).Scan(&issueID); err != nil {
		t.Fatalf("create team-assigned issue: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(ctx, `DELETE FROM issue WHERE id = $1`, issueID) })

	if err := testPool.QueryRow(ctx, `
INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority, is_leader_task, team_id)
VALUES ($1, $2, $3, 'queued', 0,
        ($1::uuid = (SELECT leader_id FROM team WHERE id = $4::uuid)),
        $4::uuid)
RETURNING id
`, agentID, runtimeID, issueID, teamID).Scan(&taskID); err != nil {
		t.Fatalf("queue task: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(ctx, `DELETE FROM agent_task_queue WHERE id = $1`, taskID) })
	return
}

// TestClaimTask_LeaderGetsBriefing — when the team leader claims a task on
// a team-assigned issue, the response's agent.instructions must include
// the Operating Protocol + Roster + user instructions.
func TestClaimTask_LeaderGetsBriefing(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()

	var leaderID, runtimeID string
	if err := testPool.QueryRow(ctx,
		`SELECT id, runtime_id FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1`,
		testWorkspaceID,
	).Scan(&leaderID, &runtimeID); err != nil {
		t.Fatalf("get leader agent: %v", err)
	}

	team := seedTeamForBriefing(t, leaderID, "Briefing Claim Team", "Be terse.")

	helper := createHandlerTestAgent(t, "Briefing Helper", []byte("[]"))
	addAgentMember(t, team.ID, helper, "implementer")

	queueTeamIssueTaskFor(t, util.UUIDToString(team.ID), leaderID, runtimeID, 95001)

	agent := claimAndDecodeAgent(t, runtimeID)
	for _, want := range []string{
		"## Team Operating Protocol",
		"## Team Roster",
		"Leader (you):",
		"## Team Instructions (Briefing Claim Team)",
		"Be terse.",
		"`[@Briefing Helper](mention://agent/" + helper + ")`",
	} {
		if !strings.Contains(agent.Instructions, want) {
			t.Errorf("expected agent.instructions to contain %q\n--- instructions ---\n%s", want, agent.Instructions)
		}
	}
}

// TestClaimTask_NonLeaderGetsNoBriefing — when a non-leader team member
// claims a task on a team-assigned issue, NO briefing is injected.
func TestClaimTask_NonLeaderGetsNoBriefing(t *testing.T) {
	if testHandler == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()

	var leaderID string
	if err := testPool.QueryRow(ctx,
		`SELECT id FROM agent WHERE workspace_id = $1 ORDER BY created_at ASC LIMIT 1`,
		testWorkspaceID,
	).Scan(&leaderID); err != nil {
		t.Fatalf("get leader agent: %v", err)
	}

	team := seedTeamForBriefing(t, leaderID, "Non-Leader Team", "Team guidance.")

	// Create a second agent (NOT the leader) with its own runtime so the
	// claim path picks its task without ambiguity.
	helperID := createHandlerTestAgent(t, "Non Leader Helper", []byte("[]"))
	addAgentMember(t, team.ID, helperID, "")
	var helperRuntime string
	if err := testPool.QueryRow(ctx,
		`SELECT runtime_id FROM agent WHERE id = $1`, helperID,
	).Scan(&helperRuntime); err != nil {
		t.Fatalf("get helper runtime: %v", err)
	}

	queueTeamIssueTaskFor(t, util.UUIDToString(team.ID), helperID, helperRuntime, 95002)

	agent := claimAndDecodeAgent(t, helperRuntime)
	for _, mustNot := range []string{
		"Team Operating Protocol",
		"Team Roster",
		"Team Instructions (Non-Leader Team)",
	} {
		if strings.Contains(agent.Instructions, mustNot) {
			t.Errorf("non-leader claim should NOT contain %q\n--- instructions ---\n%s", mustNot, agent.Instructions)
		}
	}
}

// Avoid "imported and not used: pgtype" if helpers above are the only users.
var _ pgtype.UUID
