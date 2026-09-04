package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// claimAgentInstructionsForTest claims the next queued task for runtimeID and
// returns the claimed task id, the agent Instructions carried on the claim
// response (the field the team-leader briefing is injected into), and the
// is_leader_task flag the daemon derives its team-leader role from
// (MUL-5811). Empty task id means no task was claimed.
func claimAgentInstructionsForTest(t *testing.T, runtimeID string) (taskID string, instructions string, isLeaderTask bool, raw string) {
	t.Helper()

	w := httptest.NewRecorder()
	req := newDaemonTokenRequest("POST", "/api/daemon/runtimes/"+runtimeID+"/tasks/claim", nil,
		testWorkspaceID, "team-briefing-claim")
	req = withURLParam(req, "runtimeId", runtimeID)

	testHandler.ClaimTaskByRuntime(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("ClaimTaskByRuntime: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp struct {
		Task *struct {
			ID                 string `json:"id"`
			IsLeaderTask       bool   `json:"is_leader_task"`
			LeaderRoleResolved bool   `json:"leader_role_resolved"`
			Agent              *struct {
				Instructions string `json:"instructions"`
			} `json:"agent"`
		} `json:"task"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("decode claim response: %v", err)
	}
	if resp.Task == nil {
		return "", "", false, w.Body.String()
	}
	// Every claim must advertise the capability, leader or not: its absence is
	// how a daemon detects a server too old to resolve the role, and silently
	// dropping it would send every upgraded daemon back to inferring the role
	// from instructions text — the bug MUL-5811 removed.
	if !resp.Task.LeaderRoleResolved {
		t.Fatalf("claim response must set leader_role_resolved=true: %s", w.Body.String())
	}
	var instr string
	if resp.Task.Agent != nil {
		instr = resp.Task.Agent.Instructions
	}
	return resp.Task.ID, instr, resp.Task.IsLeaderTask, w.Body.String()
}

// teamBriefingClaimFixture wires a runtime + leader agent + team and returns
// the IDs needed to enqueue leader tasks against that runtime.
type teamBriefingClaimFixture struct {
	RuntimeID string
	AgentID   string // team leader, has the runtime and empty instructions
	TeamID   string
	IssueID   string // executor_type='agent' (NOT team) — reproduces MUL-3724
}

func newTeamBriefingClaimFixture(t *testing.T, ctx context.Context, name string) teamBriefingClaimFixture {
	t.Helper()

	runtimeID := createClaimReclaimRuntime(t, ctx, name+" runtime")
	// Leader agent + an issue executed by that agent (executor_type='agent').
	agentID, issueID := createClaimReclaimAgentAndIssue(t, ctx, runtimeID, name+" leader")
	// Force empty instructions so the test asserts the briefing alone — this
	// mirrors MUL-3724 where the leader's own instructions were blank.
	if _, err := testPool.Exec(ctx, `UPDATE agent SET instructions = '' WHERE id = $1`, agentID); err != nil {
		t.Fatalf("clear leader instructions: %v", err)
	}
	// Make the issue executor an agent (NOT the team). The pre-fix code only
	// injected the briefing when issue.executor_type='team', so this is the
	// exact gap the fix closes: a comment @team-mention leader task running on
	// an issue executed by an agent.
	if _, err := testPool.Exec(ctx, `UPDATE issue SET executor_type = 'agent', executor_id = $2 WHERE id = $1`, issueID, agentID); err != nil {
		t.Fatalf("set issue agent executor: %v", err)
	}

	var teamID string
	if err := testPool.QueryRow(ctx, `
		INSERT INTO team (workspace_id, name, description, leader_id, creator_id)
		VALUES ($1, $2, '', $3, $4)
		RETURNING id
	`, testWorkspaceID, name+" team", agentID, testUserID).Scan(&teamID); err != nil {
		t.Fatalf("create team: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM team WHERE id = $1`, teamID) })

	return teamBriefingClaimFixture{
		RuntimeID: runtimeID,
		AgentID:   agentID,
		TeamID:   teamID,
		IssueID:   issueID,
	}
}

func enqueueClaimTask(t *testing.T, ctx context.Context, fx teamBriefingClaimFixture, isLeader bool, withTeamID bool) string {
	t.Helper()
	var taskID string
	var teamArg any
	if withTeamID {
		teamArg = fx.TeamID
	} else {
		teamArg = nil
	}
	if err := testPool.QueryRow(ctx, `
		INSERT INTO agent_task_queue (agent_id, runtime_id, issue_id, status, priority, is_leader_task, team_id)
		VALUES ($1, $2, $3, 'queued', 0, $4, $5)
		RETURNING id
	`, fx.AgentID, fx.RuntimeID, fx.IssueID, isLeader, teamArg).Scan(&taskID); err != nil {
		t.Fatalf("enqueue claim task: %v", err)
	}
	t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM agent_task_queue WHERE id = $1`, taskID) })
	return taskID
}

// TestClaim_LeaderTaskFromCommentMention_InjectsBriefing is the MUL-3724
// reproduction: a leader task (is_leader_task=true) carrying a team_id, on an
// issue executed by a plain AGENT (not the team). The pre-fix gate
// (issue.executor_type='team') would NOT inject the briefing here, so the
// leader booted with no team context and degraded into doing the work itself.
// After the fix the briefing is keyed off the task flag + team_id, so it is
// injected regardless of the issue executor.
func TestClaim_LeaderTaskFromCommentMention_InjectsBriefing(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fx := newTeamBriefingClaimFixture(t, ctx, "Briefing inject")
	want := enqueueClaimTask(t, ctx, fx, true /*isLeader*/, true /*withTeamID*/)

	got, instr, isLeader, raw := claimAgentInstructionsForTest(t, fx.RuntimeID)
	if got != want {
		t.Fatalf("claimed task id = %q, want %q: %s", got, want, raw)
	}
	if !strings.Contains(instr, "## Team Operating Protocol") || !strings.Contains(instr, "## Team Roster") {
		t.Fatalf("expected team-leader briefing in agent instructions, got:\n%s", instr)
	}
	// The daemon reads its leader role off this flag (MUL-5811), so an
	// injected briefing must arrive with the flag set.
	if !isLeader {
		t.Fatalf("claim injected the briefing but reported is_leader_task=false: %s", raw)
	}
}

// TestClaim_NonLeaderTask_NoBriefing guards the negative: a task that is NOT a
// leader task (is_leader_task=false), even with a team_id present, must not
// receive the briefing. This keeps worker/mention runs free of leader framing.
func TestClaim_NonLeaderTask_NoBriefing(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fx := newTeamBriefingClaimFixture(t, ctx, "Briefing nonleader")
	enqueueClaimTask(t, ctx, fx, false /*isLeader*/, true /*withTeamID*/)

	_, instr, isLeader, raw := claimAgentInstructionsForTest(t, fx.RuntimeID)
	if strings.Contains(instr, "## Team Operating Protocol") || strings.Contains(instr, "## Team Roster") {
		t.Fatalf("non-leader task must NOT get team briefing, got:\n%s", instr)
	}
	if isLeader {
		t.Fatalf("non-leader task must not report is_leader_task=true: %s", raw)
	}
}

// TestClaim_LeaderTaskWithDanglingTeamID_NoBriefing is the load-bearing
// contract for dropping the FK on agent_task_queue.team_id (migration 127):
// when a team is hard-deleted AFTER a leader task was enqueued, the task row
// keeps a now-dangling team_id. The claim must still succeed (HTTP 200, task
// delivered) and simply skip briefing injection — GetTeamInWorkspace returns
// no row, so the err != nil guard makes this identical to "condition not
// matched". Never a 500, never a stale/empty briefing. Without the FK nothing
// in the DB prevents the dangling row, so this guard lives entirely in the
// claim handler and must stay tested.
func TestClaim_LeaderTaskWithDanglingTeamID_NoBriefing(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fx := newTeamBriefingClaimFixture(t, ctx, "Briefing dangling")
	want := enqueueClaimTask(t, ctx, fx, true /*isLeader*/, true /*withTeamID*/)

	// Hard-delete the team AFTER enqueue, leaving task.team_id dangling.
	// There is no FK (migration 127), so the task row is untouched.
	if _, err := testPool.Exec(ctx, `DELETE FROM team WHERE id = $1`, fx.TeamID); err != nil {
		t.Fatalf("delete team: %v", err)
	}
	// Confirm the task still carries the (now orphaned) team_id — i.e. the
	// delete did not cascade/null it, which is the whole point of no-FK.
	var stillSet bool
	if err := testPool.QueryRow(ctx,
		`SELECT team_id = $2 FROM agent_task_queue WHERE id = $1`, want, fx.TeamID,
	).Scan(&stillSet); err != nil {
		t.Fatalf("reload task team_id: %v", err)
	}
	if !stillSet {
		t.Fatalf("expected task.team_id to remain the dangling UUID after team delete (no FK)")
	}

	got, instr, isLeader, raw := claimAgentInstructionsForTest(t, fx.RuntimeID)
	if got != want {
		t.Fatalf("claimed task id = %q, want %q (claim must still succeed 200): %s", got, want, raw)
	}
	if strings.Contains(instr, "## Team Operating Protocol") || strings.Contains(instr, "## Team Roster") {
		t.Fatalf("dangling team_id must NOT get team briefing, got:\n%s", instr)
	}
	// A leader task with no briefing has no roster to delegate to, so the
	// daemon must not run it in the leader role either (MUL-5811).
	if isLeader {
		t.Fatalf("skipped injection must clear is_leader_task on the claim response: %s", raw)
	}
}

// leader tasks enqueued before migration 127 (or by an old binary) have a NULL
// team_id. The claim handler must skip injection rather than panic or guess —
// equivalent to the pre-fix "condition not matched" behavior, never a stale
// briefing.
func TestClaim_LeaderTaskWithoutTeamID_NoBriefing(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fx := newTeamBriefingClaimFixture(t, ctx, "Briefing nullteam")
	enqueueClaimTask(t, ctx, fx, true /*isLeader*/, false /*withTeamID*/)

	_, instr, isLeader, raw := claimAgentInstructionsForTest(t, fx.RuntimeID)
	if strings.Contains(instr, "## Team Operating Protocol") || strings.Contains(instr, "## Team Roster") {
		t.Fatalf("leader task with NULL team_id must NOT get team briefing, got:\n%s", instr)
	}
	if isLeader {
		t.Fatalf("skipped injection must clear is_leader_task on the claim response: %s", raw)
	}
}

// TestClaim_LeaderSwappedAfterEnqueue_NoBriefingAndNoLeaderRole covers the
// third skip path: the team still exists, but its leader was reassigned after
// this task was enqueued, so the claiming agent is no longer the leader. The
// defensive gate already withheld the briefing; the flag must follow it down,
// otherwise the daemon would boot a former leader into the leader role with no
// roster and no protocol (MUL-5811).
func TestClaim_LeaderSwappedAfterEnqueue_NoBriefingAndNoLeaderRole(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	fx := newTeamBriefingClaimFixture(t, ctx, "Briefing swapped")
	want := enqueueClaimTask(t, ctx, fx, true /*isLeader*/, true /*withTeamID*/)

	// Hand the team to a different agent AFTER enqueue.
	otherAgentID, _ := createClaimReclaimAgentAndIssue(t, ctx, fx.RuntimeID, "Briefing swapped newleader")
	if _, err := testPool.Exec(ctx, `UPDATE team SET leader_id = $2 WHERE id = $1`, fx.TeamID, otherAgentID); err != nil {
		t.Fatalf("swap team leader: %v", err)
	}

	got, instr, isLeader, raw := claimAgentInstructionsForTest(t, fx.RuntimeID)
	if got != want {
		t.Fatalf("claimed task id = %q, want %q (claim must still succeed 200): %s", got, want, raw)
	}
	if strings.Contains(instr, "## Team Operating Protocol") || strings.Contains(instr, "## Team Roster") {
		t.Fatalf("former leader must NOT get team briefing, got:\n%s", instr)
	}
	if isLeader {
		t.Fatalf("skipped injection must clear is_leader_task on the claim response: %s", raw)
	}
}
