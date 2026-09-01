package handler

import (
	"context"
	"net/http"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

// These tests pin the authorization contract of RecordTeamLeaderEvaluation
// after MUL-6622 / GH #7487. Two gates, in order: the caller owns the task, and
// the caller is still the team's leader. The target issue's own assignee is
// deliberately not consulted.

type teamEvalFixture struct {
	TeamID      string
	LeaderID     string
	OtherID      string
	TeamIssueID string // issue assigned to the team
}

func newTeamEvalFixture(t *testing.T) teamEvalFixture {
	t.Helper()

	leaderID := createHandlerTestAgent(t, "Team Eval Leader", nil)
	otherID := createHandlerTestAgent(t, "Team Eval Other", nil)
	teamID := dbfx.Team(t, "Team Eval", leaderID)
	issueID := dbfx.Issue(t, "team eval owner issue", testutil.Cols{
		"executor_type": "team",
		"executor_id":   teamID,
	})

	return teamEvalFixture{
		TeamID:      teamID,
		LeaderID:     leaderID,
		OtherID:      otherID,
		TeamIssueID: issueID,
	}
}

// leaderTask seeds a running task bound to issueID. isLeaderTask / teamID carry
// the exact provenance under test; an empty teamID leaves team_id NULL.
func leaderTask(t *testing.T, agentID, issueID string, isLeaderTask bool, teamID string) string {
	t.Helper()

	cols := testutil.Cols{
		"runtime_id":     handlerTestRuntimeID(t),
		"status":         "running",
		"issue_id":       issueID,
		"started_at":     testutil.Raw("now()"),
		"is_leader_task": isLeaderTask,
	}
	if teamID != "" {
		cols["team_id"] = teamID
	}
	return dbfx.Task(t, agentID, cols)
}

func evaluationRequest(issueID, agentID, taskID, outcome string) *http.Request {
	return testutil.WithHeaders(
		testutil.WithURLParams(
			newRequest(http.MethodPost, "/api/issues/"+issueID+"/team-evaluated",
				map[string]any{"outcome": outcome, "reason": "test reason"}),
			"id", issueID,
		),
		"X-Agent-ID", agentID,
		"X-Task-ID", taskID,
	)
}

type recordedEvaluation struct {
	ActorID string
	TeamID string
	Outcome string
}

func loadEvaluations(t *testing.T, issueID string) []recordedEvaluation {
	t.Helper()

	rows, err := testPool.Query(context.Background(), `
		SELECT actor_id, details->>'team_id', details->>'outcome'
		FROM activity_log
		WHERE issue_id = $1 AND action = 'team_leader_evaluated'
		ORDER BY created_at ASC
	`, issueID)
	if err != nil {
		t.Fatalf("load evaluations: %v", err)
	}
	defer rows.Close()

	var out []recordedEvaluation
	for rows.Next() {
		var e recordedEvaluation
		if err := rows.Scan(&e.ActorID, &e.TeamID, &e.Outcome); err != nil {
			t.Fatalf("scan evaluation: %v", err)
		}
		out = append(out, e)
	}
	return out
}

// The regression: a leader task on an issue owned by an individual agent (the
// `@team`-mention path) used to be rejected with "issue is not assigned to a
// team", dropping the decision entirely.
func TestRecordTeamLeaderEvaluation_AcceptedOnNonTeamAssignedIssue(t *testing.T) {
	fx := newTeamEvalFixture(t)
	issueID := dbfx.Issue(t, "agent-owned issue", testutil.Cols{
		"executor_type": "agent",
		"executor_id":   fx.OtherID,
	})
	taskID := leaderTask(t, fx.LeaderID, issueID, true, fx.TeamID)

	testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(issueID, fx.LeaderID, taskID, "no_action")).Want(http.StatusCreated)

	got := loadEvaluations(t, issueID)
	if len(got) != 1 {
		t.Fatalf("expected exactly one recorded evaluation, got %d", len(got))
	}
	// actor_id must be the task's agent: the no_action comment suppression
	// lookup matches on task.agent_id.
	if got[0].ActorID != fx.LeaderID {
		t.Fatalf("actor_id: want task agent %s, got %s", fx.LeaderID, got[0].ActorID)
	}
	if got[0].TeamID != fx.TeamID {
		t.Fatalf("details.team_id: want task team %s, got %s", fx.TeamID, got[0].TeamID)
	}
	if got[0].Outcome != "no_action" {
		t.Fatalf("outcome: want no_action, got %s", got[0].Outcome)
	}
}

// A child issue the leader itself is running on records fine too — the parent's
// team assignment is irrelevant to the check.
func TestRecordTeamLeaderEvaluation_AcceptedOnChildIssueBoundTask(t *testing.T) {
	fx := newTeamEvalFixture(t)
	childID := dbfx.Issue(t, "team child issue", testutil.Cols{
		"executor_type":   "agent",
		"executor_id":     fx.OtherID,
		"parent_issue_id": fx.TeamIssueID,
	})
	taskID := leaderTask(t, fx.LeaderID, childID, true, fx.TeamID)

	testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(childID, fx.LeaderID, taskID, "action")).Want(http.StatusCreated)

	if got := loadEvaluations(t, childID); len(got) != 1 {
		t.Fatalf("expected one recorded evaluation on the child, got %d", len(got))
	}
}

// Behavior narrowing made explicit: the leader agent running a task that is NOT
// a leader task is not running as the leader, so it may not record.
func TestRecordTeamLeaderEvaluation_RejectsNonLeaderTask(t *testing.T) {
	fx := newTeamEvalFixture(t)
	taskID := leaderTask(t, fx.LeaderID, fx.TeamIssueID, false, fx.TeamID)

	testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(fx.TeamIssueID, fx.LeaderID, taskID, "no_action")).Want(http.StatusBadRequest)

	if got := loadEvaluations(t, fx.TeamIssueID); len(got) != 0 {
		t.Fatalf("expected no evaluation recorded, got %d", len(got))
	}
}

// A leader task without a stamped team cannot be attributed to a team.
func TestRecordTeamLeaderEvaluation_RejectsLeaderTaskWithoutTeamID(t *testing.T) {
	fx := newTeamEvalFixture(t)
	taskID := leaderTask(t, fx.LeaderID, fx.TeamIssueID, true, "")

	testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(fx.TeamIssueID, fx.LeaderID, taskID, "no_action")).Want(http.StatusBadRequest)

	if got := loadEvaluations(t, fx.TeamIssueID); len(got) != 0 {
		t.Fatalf("expected no evaluation recorded, got %d", len(got))
	}
}

// Recording still binds to the task's own issue, and the error names it — the
// stage-barrier case wakes the leader on the PARENT, so a leader that reaches
// for the child id gets told where to record instead of a dead end. Naming it is
// only safe because the ownership gate has already passed.
func TestRecordTeamLeaderEvaluation_RejectsCrossIssueTaskAndNamesTaskIssue(t *testing.T) {
	fx := newTeamEvalFixture(t)
	childID := dbfx.Issue(t, "stage barrier child", testutil.Cols{
		"executor_type": "agent",
		"executor_id":   fx.OtherID,
	})
	taskID := leaderTask(t, fx.LeaderID, fx.TeamIssueID, true, fx.TeamID)

	body := testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(childID, fx.LeaderID, taskID, "no_action")).Want(http.StatusBadRequest).Text()

	if !strings.Contains(body, fx.TeamIssueID) {
		t.Fatalf("expected the error to name the task's issue %s, got %q", fx.TeamIssueID, body)
	}
	if got := loadEvaluations(t, childID); len(got) != 0 {
		t.Fatalf("expected no evaluation recorded on the child, got %d", len(got))
	}
}

// An agent that is not the task's agent may not record on its behalf, and the
// rejection must not disclose anything about that task.
func TestRecordTeamLeaderEvaluation_RejectsForeignAgentWithoutLeakingTaskIssue(t *testing.T) {
	fx := newTeamEvalFixture(t)
	leaderOnly := dbfx.Issue(t, "leader-only issue", testutil.Cols{
		"executor_type": "team",
		"executor_id":   fx.TeamID,
	})
	taskID := leaderTask(t, fx.LeaderID, leaderOnly, true, fx.TeamID)

	body := testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(fx.TeamIssueID, fx.OtherID, taskID, "no_action")).Want(http.StatusForbidden).Text()

	if strings.Contains(body, leaderOnly) {
		t.Fatalf("403 body leaked the task's issue id %s: %q", leaderOnly, body)
	}
	if got := loadEvaluations(t, fx.TeamIssueID); len(got) != 0 {
		t.Fatalf("expected no evaluation recorded, got %d", len(got))
	}
}

// Tenant isolation: a task id from another workspace must be unreadable through
// an issue the caller legitimately owns, and its issue id must not appear in the
// response. GetAgentTask is a global lookup, so the workspace-scoped query plus
// the ownership gate are what stop the probe.
func TestRecordTeamLeaderEvaluation_RejectsForeignWorkspaceTaskWithoutLeakingItsIssue(t *testing.T) {
	fx := newTeamEvalFixture(t)

	foreignUser := dbfx.User(t, "Team Eval Foreign User", "team-eval-foreign@example.com")
	foreignWorkspace := dbfx.Workspace(t, "Team Eval Foreign", "team-eval-foreign")
	dbfx.Member(t, foreignWorkspace, foreignUser, "owner")
	foreignRuntime := dbfx.Runtime(t, "Team Eval Foreign Runtime", testutil.Cols{
		"workspace_id": foreignWorkspace,
		"owner_id":     foreignUser,
	})
	foreignAgent := dbfx.Agent(t, "Team Eval Foreign Agent", foreignRuntime, testutil.Cols{
		"workspace_id": foreignWorkspace,
		"owner_id":     foreignUser,
	})
	foreignTeam := dbfx.Team(t, "Team Eval Foreign Team", foreignAgent, testutil.Cols{
		"workspace_id": foreignWorkspace,
		"creator_id":   foreignUser,
	})
	foreignIssue := dbfx.Issue(t, "foreign workspace issue", testutil.Cols{
		"workspace_id":  foreignWorkspace,
		"creator_id":    foreignUser,
		"executor_type": "team",
		"executor_id":   foreignTeam,
	})
	foreignTask := dbfx.Task(t, foreignAgent, testutil.Cols{
		"runtime_id":     foreignRuntime,
		"status":         "running",
		"issue_id":       foreignIssue,
		"started_at":     testutil.Raw("now()"),
		"is_leader_task": true,
		"team_id":       foreignTeam,
	})

	body := testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(fx.TeamIssueID, fx.LeaderID, foreignTask, "no_action")).
		Want(http.StatusBadRequest).Text()

	if strings.Contains(body, foreignIssue) {
		t.Fatalf("rejection leaked a foreign workspace issue id %s: %q", foreignIssue, body)
	}
	if got := loadEvaluations(t, fx.TeamIssueID); len(got) != 0 {
		t.Fatalf("expected no evaluation recorded, got %d", len(got))
	}
}

// The claim path clears the leader role when the leader was swapped before the
// claim, but leaves is_leader_task = true on the row. The row therefore records
// enqueue-time intent, not the delivered role, so a run that was downgraded to
// an ordinary agent turn must not be able to write a leader verdict here.
func TestRecordTeamLeaderEvaluation_RejectsAfterLeaderChange(t *testing.T) {
	fx := newTeamEvalFixture(t)
	taskID := leaderTask(t, fx.LeaderID, fx.TeamIssueID, true, fx.TeamID)

	newLeader := createHandlerTestAgent(t, "Team Eval New Leader", nil)
	dbfx.Exec(t, `UPDATE team SET leader_id = $1 WHERE id = $2`, newLeader, fx.TeamID)

	testutil.Call(t, testHandler.RecordTeamLeaderEvaluation,
		evaluationRequest(fx.TeamIssueID, fx.LeaderID, taskID, "no_action")).Want(http.StatusForbidden)

	if got := loadEvaluations(t, fx.TeamIssueID); len(got) != 0 {
		t.Fatalf("expected no evaluation recorded after a leader change, got %d", len(got))
	}
}
