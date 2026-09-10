package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/google/uuid"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestTeamReviewEntryDispatchesLeader(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	for _, mode := range []string{"create", "update", "batch"} {
		t.Run(mode, func(t *testing.T) {
			runtimeID := freshReviewCoordinationRuntime(t)
			executorID := dbfx.Agent(t, "team entry executor "+mode, runtimeID)
			leaderID := dbfx.Agent(t, "team review leader "+mode, runtimeID)
			teamID := dbfx.Team(t, "review team "+mode, leaderID)
			w := httptest.NewRecorder()
			var issueID string
			if mode == "create" {
				testHandler.CreateIssue(w, newRequest(http.MethodPost, "/api/issues?workspace_id="+testWorkspaceID, map[string]any{
					"title": "team review entry", "status": "in_review",
					"executor_type": "agent", "executor_id": executorID,
					"reviewer_type": "team", "reviewer_id": teamID,
					"review_submission": reviewSubmissionFixture(),
				}))
				if w.Code != http.StatusCreated {
					t.Fatalf("create: %d %s", w.Code, w.Body.String())
				}
				var response IssueResponse
				if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
					t.Fatal(err)
				}
				issueID = response.ID
				t.Cleanup(func() { deleteTestIssue(t, issueID) })
			} else {
				issueID = dbfx.Issue(t, "team review entry", testutil.Cols{"status": "in_progress", "executor_type": "agent", "executor_id": executorID, "reviewer_type": "team", "reviewer_id": teamID})
				if mode == "update" {
					testHandler.UpdateIssue(w, withURLParam(newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
						"status": "in_review", "review_submission": reviewSubmissionFixture(),
					}), "id", issueID))
				} else {
					testHandler.BatchUpdateIssues(w, newRequest(http.MethodPost, "/api/issues/batch-update?workspace_id="+testWorkspaceID, map[string]any{
						"issue_ids": []string{issueID}, "updates": map[string]any{
							"status": "in_review", "review_submission": reviewSubmissionFixture(),
						},
					}))
				}
				if w.Code != http.StatusOK {
					t.Fatalf("transition: %d %s", w.Code, w.Body.String())
				}
			}
			cleanupIssueCoordinationRows(t, issueID)
			dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
			testHandler.AgentCoordination.RunOnce(context.Background())
			testHandler.AgentCoordination.RunOnce(context.Background())
			if got := taskCountFor(t, issueID, leaderID); got != 1 {
				t.Fatalf("leader tasks = %d, want 1", got)
			}
			var ownerType, ownerID string
			dbfx.QueryRow(t, `SELECT owner_type, owner_id::text FROM agent_coordination_assignment WHERE issue_id=$1 AND role='reviewer'`, issueID).Scan(&ownerType, &ownerID)
			if ownerType != "team" || ownerID != teamID {
				t.Fatalf("review owner = %s/%s", ownerType, ownerID)
			}
			var taskID string
			dbfx.QueryRow(t, `UPDATE agent_task_queue SET status='completed', completed_at=now() WHERE issue_id=$1 AND agent_id=$2 RETURNING id::text`, issueID, leaderID).Scan(&taskID)
			task, err := testHandler.Queries.GetAgentTask(context.Background(), parseUUID(taskID))
			if err != nil {
				t.Fatal(err)
			}
			if err := testHandler.AgentCoordination.RecordTaskCompleted(context.Background(), task); err != nil {
				t.Fatal(err)
			}
			var completions int
			dbfx.QueryRow(t, `SELECT count(*) FROM agent_coordination_outbox WHERE source_task_id=$1 AND event_key LIKE 'task_completed:%'`, taskID).Scan(&completions)
			if completions != 1 {
				t.Fatalf("team reviewer completion events = %d, want 1", completions)
			}
		})
	}
}

func TestAutomaticReviewEntryDoesNotInferExplicitSelection(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	executorID := dbfx.Agent(t, "automatic entry executor", testRuntimeID)
	reviewerID := dbfx.Agent(t, "retained reviewer without role", testRuntimeID)
	issueID := dbfx.Issue(t, "automatic entry retained reviewer", testutil.Cols{"status": "in_progress", "executor_type": "agent", "executor_id": executorID, "reviewer_type": "agent", "reviewer_id": reviewerID})
	eventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key": "automatic-entry/" + uuid.NewString(), "workspace_id": testWorkspaceID, "issue_id": issueID,
		"event_type": "task_completed", "status": "pending",
		"payload": testutil.Raw(`'{"assignment_role":"executor","agent_id":"` + executorID + `"}'::jsonb`),
	})
	dbfx.Insert(t, "agent_coordination_assignment", testutil.Cols{"event_id": eventID, "workspace_id": testWorkspaceID, "issue_id": issueID, "role": "reviewer", "status": "assigned", "owner_type": "agent", "owner_id": reviewerID})
	cleanupIssueCoordinationRows(t, issueID)
	dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
	testHandler.AgentCoordination.RunOnce(context.Background())
	if got := taskCountFor(t, issueID, reviewerID); got != 0 {
		t.Fatalf("unqualified automatic reviewer received %d tasks", got)
	}
	var status string
	dbfx.QueryRow(t, `SELECT status FROM issue WHERE id=$1`, issueID).Scan(&status)
	if status != "in_progress" {
		t.Fatalf("unqualified automatic handoff changed status to %s", status)
	}
}

func TestReviewEntryConflictsAfterConcurrentSuppressedTransition(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	executorID := dbfx.Agent(t, "suppressed entry executor", testRuntimeID)
	reviewerID := dbfx.Agent(t, "suppressed entry reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "concurrent suppressed entry", testutil.Cols{"status": "in_progress", "executor_type": "agent", "executor_id": executorID, "reviewer_type": "agent", "reviewer_id": reviewerID})
	cleanupIssueCoordinationRows(t, issueID)
	h := *testHandler
	h.Queries = db.New(&afterIssueReadDB{DBTX: testPool, afterRead: func() {
		w := httptest.NewRecorder()
		testHandler.UpdateIssue(w, withURLParam(newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
			"status": "in_review", "suppress_run": true, "review_submission": reviewSubmissionFixture(),
		}), "id", issueID))
		if w.Code != http.StatusOK {
			t.Fatalf("suppressed transition: %d %s", w.Code, w.Body.String())
		}
	}})
	w := httptest.NewRecorder()
	h.UpdateIssue(w, withURLParam(newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{"status": "in_review"}), "id", issueID))
	if w.Code != http.StatusConflict {
		t.Fatalf("concurrent unsuppressed transition: %d %s, want 409", w.Code, w.Body.String())
	}
}

func TestMemberReviewCreateWithNilCoordinator(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	executorID := dbfx.Agent(t, "member review executor", testRuntimeID)
	tasks := *testHandler.TaskService
	tasks.Coordination = nil
	issues := *testHandler.IssueService
	issues.TaskService = &tasks
	h := *testHandler
	h.IssueService = &issues
	w := httptest.NewRecorder()
	h.CreateIssue(w, newRequest(http.MethodPost, "/api/issues?workspace_id="+testWorkspaceID, map[string]any{
		"title": "human review without coordinator", "status": "in_review",
		"executor_type": "agent", "executor_id": executorID,
		"reviewer_type": "member", "reviewer_id": testUserID,
		"review_submission": reviewSubmissionFixture(),
	}))
	if w.Code != http.StatusCreated {
		t.Fatalf("member review create: %d %s", w.Code, w.Body.String())
	}
	var response IssueResponse
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { deleteTestIssue(t, response.ID) })
}
