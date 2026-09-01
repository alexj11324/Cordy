package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

type runningTeamLeaderTaskFixture struct {
	IssueID          string
	TeamID          string
	LeaderID         string
	TaskID           string
	TriggerCommentID string
}

func newRunningTeamLeaderTaskFixture(t *testing.T) runningTeamLeaderTaskFixture {
	t.Helper()

	fx := newTeamCommentTriggerFixture(t)
	issueID := uuidToString(fx.Issue.ID)

	var runtimeID string
	dbfx.QueryRow(t, `SELECT runtime_id FROM agent WHERE id = $1`, fx.LeaderID).Scan(&runtimeID)

	triggerCommentID := dbfx.Comment(t, issueID, "LGTM")

	// is_leader_task + team_id are what RecordTeamLeaderEvaluation authorizes
	// against (MUL-6622); a leader task without them is not a leader turn.
	taskID := dbfx.Task(t, fx.LeaderID, testutil.Cols{
		"runtime_id":         runtimeID,
		"issue_id":           issueID,
		"trigger_comment_id": triggerCommentID,
		"status":             "running",
		"started_at":         testutil.Raw("now()"),
		"is_leader_task":     true,
		"team_id":           fx.TeamID,
	})

	return runningTeamLeaderTaskFixture{
		IssueID:          issueID,
		TeamID:          fx.TeamID,
		LeaderID:         fx.LeaderID,
		TaskID:           taskID,
		TriggerCommentID: triggerCommentID,
	}
}

func recordTeamLeaderEvaluationForTask(t *testing.T, fx runningTeamLeaderTaskFixture, outcome string) {
	t.Helper()
	recordTeamLeaderEvaluationForTaskWithHeader(t, fx, outcome, fx.TaskID)
}

func recordTeamLeaderEvaluationForTaskWithHeader(t *testing.T, fx runningTeamLeaderTaskFixture, outcome, taskIDHeader string) {
	t.Helper()

	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/issues/"+fx.IssueID+"/team-evaluated", map[string]any{
		"outcome": outcome,
		"reason":  "test reason",
	})
	r = withURLParam(r, "id", fx.IssueID)
	r.Header.Set("X-Agent-ID", fx.LeaderID)
	r.Header.Set("X-Task-ID", taskIDHeader)

	testHandler.RecordTeamLeaderEvaluation(w, r)
	if w.Code != http.StatusCreated {
		t.Fatalf("RecordTeamLeaderEvaluation: expected 201, got %d: %s", w.Code, w.Body.String())
	}
}

func completeRunningTask(t *testing.T, fx runningTeamLeaderTaskFixture, output string) {
	t.Helper()

	w := httptest.NewRecorder()
	r := newDaemonTokenRequest("POST", "/api/daemon/tasks/"+fx.TaskID+"/complete",
		map[string]any{"output": output},
		testWorkspaceID, "legit-daemon")
	rctx := chi.NewRouteContext()
	rctx.URLParams.Add("taskId", fx.TaskID)
	r = r.WithContext(context.WithValue(r.Context(), chi.RouteCtxKey, rctx))

	testHandler.CompleteTask(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("CompleteTask: expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

func countAgentCommentsForIssue(t *testing.T, issueID, agentID string) int {
	t.Helper()
	var count int
	if err := testPool.QueryRow(context.Background(), `
		SELECT count(*) FROM comment
		WHERE issue_id = $1 AND author_type = 'agent' AND author_id = $2
	`, issueID, agentID).Scan(&count); err != nil {
		t.Fatalf("count agent comments: %v", err)
	}
	return count
}

func TestCompleteTask_TeamLeaderNoActionDoesNotSynthesizeComment(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	fx := newRunningTeamLeaderTaskFixture(t)
	recordTeamLeaderEvaluationForTask(t, fx, "no_action")

	completeRunningTask(t, fx, "No action needed. Exiting silently.")

	if got := countAgentCommentsForIssue(t, fx.IssueID, fx.LeaderID); got != 0 {
		t.Fatalf("expected no team leader comment after no_action completion, got %d", got)
	}
}

func TestCompleteTask_TeamLeaderNoActionCanonicalizesTaskID(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	fx := newRunningTeamLeaderTaskFixture(t)
	recordTeamLeaderEvaluationForTaskWithHeader(t, fx, "no_action", strings.ToUpper(fx.TaskID))

	completeRunningTask(t, fx, "No action needed. Exiting silently.")

	if got := countAgentCommentsForIssue(t, fx.IssueID, fx.LeaderID); got != 0 {
		t.Fatalf("expected no comment when no_action was recorded with uppercase task id header, got %d", got)
	}
}

func TestCompleteTask_TeamLeaderActionStillSynthesizesComment(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	fx := newRunningTeamLeaderTaskFixture(t)
	recordTeamLeaderEvaluationForTask(t, fx, "action")

	completeRunningTask(t, fx, "Delegated the review.")

	if got := countAgentCommentsForIssue(t, fx.IssueID, fx.LeaderID); got != 1 {
		t.Fatalf("expected action completion to synthesize one comment, got %d", got)
	}
}

func TestCreateComment_TeamLeaderNoActionRejectsComment(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	fx := newRunningTeamLeaderTaskFixture(t)
	recordTeamLeaderEvaluationForTask(t, fx, "no_action")

	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/issues/"+fx.IssueID+"/comments", map[string]any{
		"content":   "No action needed.",
		"parent_id": fx.TriggerCommentID,
	})
	r = withURLParam(r, "id", fx.IssueID)
	r.Header.Set("X-Agent-ID", fx.LeaderID)
	r.Header.Set("X-Task-ID", fx.TaskID)

	testHandler.CreateComment(w, r)
	if w.Code != http.StatusConflict {
		t.Fatalf("CreateComment: expected 409, got %d: %s", w.Code, w.Body.String())
	}
	if got := countAgentCommentsForIssue(t, fx.IssueID, fx.LeaderID); got != 0 {
		t.Fatalf("expected rejected no_action comment not to be stored, got %d", got)
	}

	var body map[string]any
	if err := json.NewDecoder(w.Body).Decode(&body); err != nil {
		t.Fatalf("decode error response: %v", err)
	}
	if body["error"] == "" {
		t.Fatalf("expected error message in response, got %v", body)
	}
}
