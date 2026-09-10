package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/orvilo-ai/orvilo/server/internal/events"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

type afterIssueReadDB struct {
	db.DBTX
	afterRead func()
}

func (d *afterIssueReadDB) QueryRow(ctx context.Context, query string, args ...any) pgx.Row {
	row := d.DBTX.QueryRow(ctx, query, args...)
	if strings.HasPrefix(query, "-- name: GetIssueInWorkspace :one") && d.afterRead != nil {
		afterRead := d.afterRead
		d.afterRead = nil
		return afterIssueReadRow{Row: row, afterRead: afterRead}
	}
	return row
}

type afterIssueReadRow struct {
	pgx.Row
	afterRead func()
}

func freshReviewCoordinationRuntime(t *testing.T) string {
	t.Helper()
	return createRuntimeLocalSkillTestRuntime(t, testUserID)
}

func (r afterIssueReadRow) Scan(dest ...any) error {
	if err := r.Row.Scan(dest...); err != nil {
		return err
	}
	r.afterRead()
	return nil
}

func TestIssueMetadataUpdatePreservesConcurrentReviewHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	executor := dbfx.Agent(t, "metadata executor", testRuntimeID)
	previousReviewer := dbfx.Agent(t, "old reviewer", testRuntimeID)
	nextReviewer := dbfx.Agent(t, "new reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "metadata review race", testutil.Cols{
		"status": "in_progress", "executor_type": "agent", "executor_id": executor,
		"reviewer_type": "agent", "reviewer_id": previousReviewer,
	})
	cleanupIssueCoordinationRows(t, issueID)
	h := *testHandler
	h.Queries = db.New(&afterIssueReadDB{DBTX: testPool, afterRead: func() {
		// Commit the other request after the metadata writer reads its snapshot.
		w := httptest.NewRecorder()
		r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
			"status": "in_review", "reviewer_type": "agent", "reviewer_id": nextReviewer,
			"review_submission": reviewSubmissionFixture(),
		})
		testHandler.UpdateIssue(w, withURLParam(r, "id", issueID))
		if w.Code != http.StatusOK {
			t.Fatalf("concurrent handoff: %d %s", w.Code, w.Body.String())
		}
	}})
	w := httptest.NewRecorder()
	r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{"priority": "high"})
	h.UpdateIssue(w, withURLParam(r, "id", issueID))
	if w.Code != http.StatusOK {
		t.Fatalf("metadata update: %d %s", w.Code, w.Body.String())
	}
	var reviewer, status, priority string
	dbfx.QueryRow(t, `SELECT reviewer_id::text, status, priority FROM issue WHERE id = $1`, issueID).Scan(&reviewer, &status, &priority)
	if reviewer != nextReviewer || status != "in_review" || priority != "high" {
		t.Fatalf("metadata overwrote review handoff: reviewer=%s status=%s priority=%s", reviewer, status, priority)
	}
}

func TestBatchReviewerOnlyChangeDispatchesNewReviewer(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	executor := dbfx.Agent(t, "batch executor", testRuntimeID)
	previousReviewer := dbfx.Agent(t, "batch old reviewer", testRuntimeID)
	nextReviewer := dbfx.Agent(t, "batch new reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "batch review reassignment", testutil.Cols{
		"status": "in_review", "executor_type": "agent", "executor_id": executor,
		"reviewer_type": "agent", "reviewer_id": previousReviewer,
		"review_submission": reviewSubmissionDBFixture(),
	})
	cleanupIssueCoordinationRows(t, issueID)
	w := httptest.NewRecorder()
	testHandler.BatchUpdateIssues(w, newRequest(http.MethodPut, "/api/issues/batch?workspace_id="+testWorkspaceID, map[string]any{
		"issue_ids": []string{issueID}, "updates": map[string]any{"reviewer_type": "agent", "reviewer_id": nextReviewer},
	}))
	if w.Code != http.StatusOK {
		t.Fatalf("batch reviewer: %d %s", w.Code, w.Body.String())
	}
	var assignments int
	dbfx.QueryRow(t, `SELECT count(*) FROM agent_coordination_assignment WHERE issue_id = $1 AND owner_id = $2`, issueID, nextReviewer).Scan(&assignments)
	if assignments != 1 {
		t.Fatalf("new reviewer assignments = %d; response=%s", assignments, w.Body.String())
	}
}

func TestAutomaticReviewerStillNeedsReviewRoleBeforeDispatch(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	executor := dbfx.Agent(t, "automatic executor", testRuntimeID)
	reviewer := dbfx.Agent(t, "former automatic reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "automatic reviewer lost role", testutil.Cols{
		"status": "in_review", "executor_type": "agent", "executor_id": executor,
		"reviewer_type": "agent", "reviewer_id": reviewer,
		"review_submission": reviewSubmissionDBFixture(),
	})
	eventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key": "automatic-reviewer-role/" + uuid.NewString(), "workspace_id": testWorkspaceID,
		"issue_id": issueID, "event_type": "task_completed", "status": "pending",
		"payload": testutil.Raw(`'{"assignment_role":"reviewer","explicit_reviewer":false}'::jsonb`),
	})
	dbfx.Insert(t, "agent_coordination_assignment", testutil.Cols{
		"event_id": eventID, "workspace_id": testWorkspaceID, "issue_id": issueID,
		"role": "reviewer", "status": "assigned", "owner_type": "agent", "owner_id": reviewer,
	})
	dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
	testHandler.AgentCoordination.RunOnce(context.Background())
	if got := taskCountFor(t, issueID, reviewer); got != 0 {
		t.Fatalf("automatic reviewer without a review role received %d tasks", got)
	}
}

func TestAutomaticReviewWithoutSubmissionBlocksAssignment(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	runtimeID := freshReviewCoordinationRuntime(t)
	executorID := dbfx.Agent(t, "missing submission executor", runtimeID)
	reviewerID := dbfx.Agent(t, "missing submission reviewer", runtimeID)
	issueID := dbfx.Issue(t, "automatic review missing submission", testutil.Cols{
		"status":        "in_progress",
		"executor_type": "agent",
		"executor_id":   executorID,
	})
	dbfx.Exec(t, `
		INSERT INTO workspace_issue_category_policy (workspace_id, category, default_reviewer_agent_id)
		VALUES ($1, 'in_review', $2)
		ON CONFLICT (workspace_id, category) DO UPDATE
		SET default_reviewer_agent_id = EXCLUDED.default_reviewer_agent_id, updated_at = now()
	`, testWorkspaceID, reviewerID)
	dbfx.Cleanup(t, `DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1 AND category = 'in_review'`, testWorkspaceID)

	sourceTaskID := dbfx.Task(t, executorID, testutil.Cols{
		"runtime_id":   runtimeID,
		"issue_id":     issueID,
		"status":       "completed",
		"completed_at": testutil.Raw("now()"),
		"context":      testutil.Raw("'{}'::jsonb"),
	})
	eventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key":      "missing-review-submission/" + uuid.NewString(),
		"workspace_id":   testWorkspaceID,
		"issue_id":       issueID,
		"source_task_id": sourceTaskID,
		"event_type":     "task_completed",
		"status":         "pending",
		"payload":        testutil.Raw(`'{"assignment_role":"executor","agent_id":"` + executorID + `"}'::jsonb`),
	})
	assignmentID := dbfx.Insert(t, "agent_coordination_assignment", testutil.Cols{
		"event_id":       eventID,
		"workspace_id":   testWorkspaceID,
		"issue_id":       issueID,
		"source_task_id": sourceTaskID,
		"role":           "reviewer",
		"status":         "assigned",
	})
	dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
	cleanupIssueCoordinationRows(t, issueID)

	testHandler.AgentCoordination.RunOnce(context.Background())

	var issueStatus string
	var reviewerIDValue *string
	dbfx.QueryRow(t, `SELECT status, reviewer_id::text FROM issue WHERE id = $1`, issueID).Scan(&issueStatus, &reviewerIDValue)
	if issueStatus != "in_progress" || reviewerIDValue != nil {
		t.Fatalf("missing submission changed issue to %q with reviewer %v", issueStatus, reviewerIDValue)
	}
	if got := taskCountFor(t, issueID, reviewerID); got != 0 {
		t.Fatalf("missing submission dispatched %d reviewer tasks", got)
	}

	var eventStatus, assignmentStatus, lastError, outcome, reason string
	dbfx.QueryRow(t, `SELECT status FROM agent_coordination_outbox WHERE id = $1`, eventID).Scan(&eventStatus)
	dbfx.QueryRow(t, `
		SELECT status, COALESCE(last_error, ''), decision->>'outcome', decision->>'reason'
		FROM agent_coordination_assignment WHERE id = $1
	`, assignmentID).Scan(&assignmentStatus, &lastError, &outcome, &reason)
	if eventStatus != "completed" || assignmentStatus != "blocked" || outcome != "blocked" || reason != "review_submission_required" || !strings.Contains(lastError, "review_submission_required") {
		t.Fatalf("missing submission decision = event %q assignment %q outcome %q reason %q error %q", eventStatus, assignmentStatus, outcome, reason, lastError)
	}
}

func TestReviewCannotClearReviewerOrAssignExecutorAsReviewer(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	executor := dbfx.Agent(t, "invariant executor", testRuntimeID)
	reviewer := dbfx.Agent(t, "invariant reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "review role invariant", testutil.Cols{
		"status": "in_review", "executor_type": "agent", "executor_id": executor,
		"reviewer_type": "agent", "reviewer_id": reviewer,
		"review_submission": reviewSubmissionDBFixture(),
	})
	for _, body := range []map[string]any{
		{"reviewer_type": nil, "reviewer_id": nil, "suppress_run": true},
		{"executor_type": "agent", "executor_id": reviewer},
	} {
		w := httptest.NewRecorder()
		r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, body)
		testHandler.UpdateIssue(w, withURLParam(r, "id", issueID))
		if w.Code != http.StatusBadRequest {
			t.Fatalf("invalid review roles accepted: %d %s", w.Code, w.Body.String())
		}
	}
}

func TestReviewEntryRejectsUnknownOrPrivateReviewer(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	executor := dbfx.Agent(t, "authorized executor", testRuntimeID)
	otherOwner := dbfx.User(t, "Reviewer Owner", "reviewer-owner-"+uuid.NewString()+"@test.local")
	privateReviewer := dbfx.Agent(t, "private reviewer", testRuntimeID, testutil.Cols{"owner_id": otherOwner, "permission_mode": "private"})
	for _, reviewer := range []string{uuid.NewString(), privateReviewer} {
		issueID := dbfx.Issue(t, "unauthorized review entry", testutil.Cols{
			"status": "in_progress", "executor_type": "agent", "executor_id": executor,
		})
		w := httptest.NewRecorder()
		r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
			"status": "in_review", "reviewer_type": "agent", "reviewer_id": reviewer,
		})
		testHandler.UpdateIssue(w, withURLParam(r, "id", issueID))
		if w.Code != http.StatusBadRequest && w.Code != http.StatusForbidden {
			t.Fatalf("invalid reviewer accepted: %d %s", w.Code, w.Body.String())
		}
		var status string
		dbfx.QueryRow(t, `SELECT status FROM issue WHERE id = $1`, issueID).Scan(&status)
		if status != "in_progress" {
			t.Fatalf("refused review changed status to %s", status)
		}
	}
}

func TestIssueReviewEntryRecordsDurableReviewerHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })
	for _, mode := range []string{"update", "batch", "create"} {
		t.Run(mode, func(t *testing.T) {
			runtimeID := freshReviewCoordinationRuntime(t)
			executorID := dbfx.Agent(t, "entry-executor-"+mode, runtimeID)
			reviewerID := dbfx.Agent(t, "entry-reviewer-"+mode, runtimeID)
			var issueID string
			w := httptest.NewRecorder()
			if mode == "create" {
				testHandler.CreateIssue(w, newRequest(http.MethodPost, "/api/issues?workspace_id="+testWorkspaceID, map[string]any{
					"title": "new review handoff", "status": "in_review",
					"executor_type": "agent", "executor_id": executorID,
					"reviewer_type": "agent", "reviewer_id": reviewerID,
					"review_submission": reviewSubmissionFixture(),
				}))
				if w.Code != http.StatusCreated {
					t.Fatalf("create review issue: %d %s", w.Code, w.Body.String())
				}
				var response IssueResponse
				if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
					t.Fatal(err)
				}
				issueID = response.ID
				t.Cleanup(func() { deleteTestIssue(t, issueID) })
			} else {
				issueID = dbfx.Issue(t, "review entry "+mode, testutil.Cols{
					"status": "in_progress", "executor_type": "agent", "executor_id": executorID,
					"reviewer_type": "agent", "reviewer_id": reviewerID,
				})
				if mode == "update" {
					r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
						"status": "in_review", "review_submission": reviewSubmissionFixture(),
					})
					testHandler.UpdateIssue(w, withURLParam(r, "id", issueID))
				} else {
					testHandler.BatchUpdateIssues(w, newRequest(http.MethodPut, "/api/issues/batch?workspace_id="+testWorkspaceID, map[string]any{
						"issue_ids": []string{issueID}, "updates": map[string]any{
							"status": "in_review", "review_submission": reviewSubmissionFixture(),
						},
					}))
				}
				if w.Code != http.StatusOK {
					t.Fatalf("enter review: %d %s", w.Code, w.Body.String())
				}
			}
			cleanupIssueCoordinationRows(t, issueID)
			dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
			var assignments int
			dbfx.QueryRow(t, `SELECT count(*) FROM agent_coordination_assignment WHERE issue_id = $1 AND role = 'reviewer' AND owner_id = $2 AND status = 'assigned'`, issueID, reviewerID).Scan(&assignments)
			if assignments != 1 {
				t.Fatalf("reviewer dispatch obligations = %d, want 1", assignments)
			}
			testHandler.AgentCoordination.RunOnce(context.Background())
			testHandler.AgentCoordination.RunOnce(context.Background())
			if got := taskCountFor(t, issueID, reviewerID); got != 1 {
				t.Fatalf("reviewer tasks = %d, want 1", got)
			}
			if got := taskCountFor(t, issueID, executorID); got != 0 {
				t.Fatalf("implementation restarted during review: %d tasks", got)
			}
		})
	}
}

func TestUpdateIssue_ReviewReturnRetiresReviewerTaskAndRecordsExecutorHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })

	executorID := dbfx.Agent(t, "review-return-executor", testRuntimeID)
	reviewerID := dbfx.Agent(t, "review-return-reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "review return coordination", testutil.Cols{
		"status":            "in_review",
		"executor_type":     "agent",
		"executor_id":       executorID,
		"reviewer_type":     "agent",
		"reviewer_id":       reviewerID,
		"review_submission": reviewSubmissionDBFixture(),
	})
	reviewerTaskID := seedDispatchedReviewerCoordinationTask(t, issueID, reviewerID)
	cleanupIssueCoordinationRows(t, issueID)

	w := httptest.NewRecorder()
	r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
		"status":       "in_progress",
		"handoff_note": "address the requested changes",
	})
	r = withURLParam(r, "id", issueID)
	testHandler.UpdateIssue(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("review return: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var issueStatus, taskStatus string
	dbfx.QueryRow(t, `SELECT status FROM issue WHERE id = $1`, issueID).Scan(&issueStatus)
	dbfx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id = $1`, reviewerTaskID).Scan(&taskStatus)
	if issueStatus != "in_progress" || taskStatus != "cancelled" {
		t.Fatalf("committed state = issue %q, reviewer task %q; want in_progress/cancelled", issueStatus, taskStatus)
	}

	var eventType, sourceTaskID, outcome, handoffNote, role, ownerType, ownerID, assignmentStatus string
	dbfx.QueryRow(t, `
		SELECT event.event_type,
		       event.source_task_id::text,
		       event.payload->>'outcome',
		       event.payload->>'handoff_note',
		       assignment.role,
		       assignment.owner_type,
		       assignment.owner_id::text,
		       assignment.status
		FROM agent_coordination_outbox AS event
		JOIN agent_coordination_assignment AS assignment ON assignment.event_id = event.id
		WHERE event.issue_id = $1 AND event.event_key LIKE 'review_returned:%'
	`, issueID).Scan(&eventType, &sourceTaskID, &outcome, &handoffNote, &role, &ownerType, &ownerID, &assignmentStatus)
	if eventType != "review_returned" || sourceTaskID != reviewerTaskID || outcome != "review_returned" {
		t.Fatalf("review return event = type %q source %q outcome %q", eventType, sourceTaskID, outcome)
	}
	if handoffNote != "address the requested changes" {
		t.Fatalf("handoff note = %q", handoffNote)
	}
	if role != "executor" || ownerType != "agent" || ownerID != executorID || assignmentStatus != "assigned" {
		t.Fatalf("executor handoff = role %q owner %q/%q status %q", role, ownerType, ownerID, assignmentStatus)
	}
	assertNoActiveIssueTasks(t, issueID)
}

func TestUpdateIssue_ReviewerReassignmentRetiresOldTaskAndRecordsExplicitReviewer(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })

	executorID := dbfx.Agent(t, "reviewer-reassign-executor", testRuntimeID)
	previousReviewerID := dbfx.Agent(t, "previous-reviewer", testRuntimeID)
	nextReviewerID := dbfx.Agent(t, "next-reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "reviewer reassignment coordination", testutil.Cols{
		"status":            "in_review",
		"executor_type":     "agent",
		"executor_id":       executorID,
		"reviewer_type":     "agent",
		"reviewer_id":       previousReviewerID,
		"review_submission": reviewSubmissionDBFixture(),
	})
	reviewerTaskID := seedDispatchedReviewerCoordinationTask(t, issueID, previousReviewerID)
	cleanupIssueCoordinationRows(t, issueID)

	w := httptest.NewRecorder()
	r := newRequest(http.MethodPut, "/api/issues/"+issueID+"?workspace_id="+testWorkspaceID, map[string]any{
		"reviewer_type": "agent",
		"reviewer_id":   nextReviewerID,
	})
	r = withURLParam(r, "id", issueID)
	testHandler.UpdateIssue(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("reviewer reassignment: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var reviewerID, taskStatus string
	dbfx.QueryRow(t, `SELECT reviewer_id::text FROM issue WHERE id = $1`, issueID).Scan(&reviewerID)
	dbfx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id = $1`, reviewerTaskID).Scan(&taskStatus)
	if reviewerID != nextReviewerID || taskStatus != "cancelled" {
		t.Fatalf("committed state = reviewer %q, old task %q; want %q/cancelled", reviewerID, taskStatus, nextReviewerID)
	}

	var eventType, sourceTaskID, outcome, role, ownerType, ownerID, assignmentStatus string
	dbfx.QueryRow(t, `
		SELECT event.event_type,
		       event.source_task_id::text,
		       event.payload->>'outcome',
		       assignment.role,
		       assignment.owner_type,
		       assignment.owner_id::text,
		       assignment.status
		FROM agent_coordination_outbox AS event
		JOIN agent_coordination_assignment AS assignment ON assignment.event_id = event.id
		WHERE event.issue_id = $1 AND event.event_key LIKE 'reviewer_reassigned:%'
	`, issueID).Scan(&eventType, &sourceTaskID, &outcome, &role, &ownerType, &ownerID, &assignmentStatus)
	if eventType != "task_completed" || sourceTaskID != reviewerTaskID || outcome != "reviewer_reassigned" {
		t.Fatalf("reviewer handoff event = type %q source %q outcome %q", eventType, sourceTaskID, outcome)
	}
	if role != "reviewer" || ownerType != "agent" || ownerID != nextReviewerID || assignmentStatus != "assigned" {
		t.Fatalf("reviewer handoff = role %q owner %q/%q status %q", role, ownerType, ownerID, assignmentStatus)
	}
	assertNoActiveIssueTasks(t, issueID)
}

func TestAgentCoordinationRunOnceSelectsReviewerAndPublishesHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)

	runtimeID := freshReviewCoordinationRuntime(t)
	executorID := dbfx.Agent(t, "coordination implementation", runtimeID)
	reviewerID := dbfx.Agent(t, "coordination reviewer", runtimeID)
	issueID := dbfx.Issue(t, "coordination reviewer selection", testutil.Cols{
		"status":        "in_progress",
		"executor_type": "agent",
		"executor_id":   executorID,
	})
	dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
	dbfx.Exec(t, `
		INSERT INTO workspace_issue_category_policy (workspace_id, category, default_reviewer_agent_id)
		VALUES ($1, 'in_review', $2)
		ON CONFLICT (workspace_id, category) DO UPDATE
		SET default_reviewer_agent_id = EXCLUDED.default_reviewer_agent_id, updated_at = now()
	`, testWorkspaceID, reviewerID)
	dbfx.Cleanup(t, `DELETE FROM workspace_issue_category_policy WHERE workspace_id = $1 AND category = 'in_review'`, testWorkspaceID)

	// Seed the completed implementation task and its original executor
	// assignment. RecordTaskCompleted is the producer boundary under test; the
	// coordinator worker must then create the reviewer assignment and task.
	sourceTaskID := dbfx.Task(t, executorID, testutil.Cols{
		"runtime_id":   runtimeID,
		"issue_id":     issueID,
		"status":       "completed",
		"completed_at": testutil.Raw("now()"),
		"context":      testutil.Raw("'{}'::jsonb"),
		"result": testutil.Raw(`'{
			"execution_repo_identity":"acme/coordination-review",
			"execution_workspace":"/srv/coordination-review",
			"execution_head_branch":"agent/coordination-review",
			"execution_head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
			"execution_head_state":"attached"
		}'::jsonb`),
	})
	// Automatic review entry is allowed only when the completed task's
	// attested checkout resolves to a real provider PR. Seed the same durable
	// provenance and Work Product relation the terminal/discovery paths create.
	const (
		reviewRepo     = "acme/coordination-review"
		reviewBranch   = "agent/coordination-review"
		reviewHeadSHA  = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
		reviewWorktree = "/srv/coordination-review"
		reviewPRURL    = "https://github.com/acme/coordination-review/pull/42"
	)
	dbfx.Exec(t, `
		INSERT INTO agent_task_execution_provenance (
			workspace_id, task_id, repo_identity, execution_workspace,
			head_branch, head_sha, head_state, finished_at, discovery_status,
			discovery_at
		) VALUES ($1, $2, $3, $4, $5, $6, 'attached', now(), 'associated', now())
	`, testWorkspaceID, sourceTaskID, reviewRepo, reviewWorktree, reviewBranch, reviewHeadSHA)
	// A later discovery update for a different checkout must not replace the
	// terminal checkout selected from task.result. GetProvenanceByTask used to
	// order by updated_at, so this row reproduced a false block when it had no
	// matching PR of its own.
	dbfx.Exec(t, `
		INSERT INTO agent_task_execution_provenance (
			workspace_id, task_id, repo_identity, execution_workspace,
			head_branch, head_sha, head_state, finished_at, discovery_status,
			discovery_at, updated_at
		) VALUES ($1, $2, $3, $4, $5, $6, 'attached', now(), 'associated', now(), now() + interval '1 hour')
	`, testWorkspaceID, sourceTaskID, "acme/older-checkout", "/srv/older-checkout", "agent/older-checkout", strings.Repeat("b", 40))
	dbfx.Cleanup(t, `DELETE FROM agent_task_execution_provenance WHERE workspace_id = $1 AND task_id = $2`, testWorkspaceID, sourceTaskID)
	prID := dbfx.Insert(t, "github_pull_request", testutil.Cols{
		"workspace_id":    testWorkspaceID,
		"installation_id": int64(42),
		"repo_owner":      "acme",
		"repo_name":       "coordination-review",
		"pr_number":       int32(42),
		"title":           "Coordination review evidence",
		"state":           "open",
		"html_url":        reviewPRURL,
		"branch":          reviewBranch,
		"pr_created_at":   testutil.Raw("now()"),
		"pr_updated_at":   testutil.Raw("now()"),
		"head_sha":        reviewHeadSHA,
	})
	productID := dbfx.Insert(t, "work_product", testutil.Cols{
		"workspace_id":         testWorkspaceID,
		"kind":                 "pull_request",
		"provider":             "github",
		"external_identity":    "acme/coordination-review#42",
		"external_url":         reviewPRURL,
		"provider_record_type": "github_pull_request",
		"provider_record_id":   prID,
	})
	dbfx.Insert(t, "work_product_relation", testutil.Cols{
		"workspace_id":     testWorkspaceID,
		"work_product_id":  productID,
		"issue_id":         issueID,
		"task_id":          sourceTaskID,
		"relation_key":     "coordination-review-evidence/" + uuid.NewString(),
		"relation_source":  "execution_branch_discovery",
		"attached_by_type": "agent",
		"attached_by_id":   executorID,
	})
	sourceEventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key":      "coordination-source-" + uuid.NewString(),
		"workspace_id":   testWorkspaceID,
		"issue_id":       issueID,
		"source_task_id": nil,
		"event_type":     "task_completed",
		"status":         "completed",
		"payload":        testutil.Raw("'{}'::jsonb"),
	})
	assignmentID := dbfx.Insert(t, "agent_coordination_assignment", testutil.Cols{
		"event_id":           sourceEventID,
		"workspace_id":       testWorkspaceID,
		"issue_id":           issueID,
		"source_task_id":     sourceTaskID,
		"role":               "executor",
		"status":             "dispatched",
		"owner_type":         "agent",
		"owner_id":           executorID,
		"dispatched_task_id": sourceTaskID,
	})
	dbfx.Exec(t, `
		UPDATE agent_task_queue
		SET context = jsonb_build_object(
			'coordination_assignment_id', $1::text,
			'coordination_assignment_role', 'executor',
			'coordination_owner_type', 'agent',
			'coordination_owner_id', $2::text
		)
		WHERE id = $3
	`, assignmentID, executorID, sourceTaskID)
	cleanupIssueCoordinationRows(t, issueID)

	var handoff events.Event
	testHandler.Bus.Subscribe(protocol.EventIssueUpdated, func(event events.Event) {
		payload, ok := event.Payload.(map[string]any)
		if !ok || payload["coordination_event_id"] == nil {
			return
		}
		issue, ok := payload["issue"].(map[string]any)
		if ok && issue["id"] == issueID && payload["review_handoff"] == true {
			handoff = event
		}
	})

	task, err := testHandler.Queries.GetAgentTask(context.Background(), parseUUID(sourceTaskID))
	if err != nil {
		t.Fatalf("load completed implementation task: %v", err)
	}
	if err := testHandler.AgentCoordination.RecordTaskCompleted(context.Background(), task); err != nil {
		t.Fatalf("record completed implementation task: %v", err)
	}
	testHandler.AgentCoordination.RunOnce(context.Background())

	var issueStatus, gotReviewer string
	dbfx.QueryRow(t, `SELECT status, reviewer_id::text FROM issue WHERE id = $1`, issueID).Scan(&issueStatus, &gotReviewer)
	if issueStatus != "in_review" || gotReviewer != reviewerID {
		t.Fatalf("coordinator issue state = %q/%q; want in_review/%q", issueStatus, gotReviewer, reviewerID)
	}
	var reviewerTasks int
	dbfx.QueryRow(t, `
		SELECT count(*) FROM agent_task_queue
		WHERE issue_id = $1 AND agent_id = $2
		  AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
	`, issueID, reviewerID).Scan(&reviewerTasks)
	if reviewerTasks != 1 {
		t.Fatalf("reviewer active tasks = %d, want 1", reviewerTasks)
	}
	if handoff.Type != protocol.EventIssueUpdated {
		t.Fatalf("handoff event = %+v, want issue:updated", handoff)
	}
	handoffPayload := handoff.Payload.(map[string]any)
	if handoffPayload["coordination_publication"] != "review_handoff" || handoffPayload["coordination_event_id"] == "" || handoffPayload["coordination_publication_key"] == "" {
		t.Fatalf("handoff publication metadata = %#v", handoffPayload)
	}
}

func TestAgentCoordinationRunOnceRecoversUnpublishedReviewHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)

	runtimeID := freshReviewCoordinationRuntime(t)
	executorID := dbfx.Agent(t, "coordination recovery executor", runtimeID)
	reviewerID := dbfx.Agent(t, "coordination recovery reviewer", runtimeID)
	issueID := dbfx.Issue(t, "coordination recovery", testutil.Cols{
		"status":            "in_review",
		"executor_type":     "agent",
		"executor_id":       executorID,
		"reviewer_type":     "agent",
		"reviewer_id":       reviewerID,
		"review_submission": reviewSubmissionDBFixture(),
	})
	dbfx.Cleanup(t, `DELETE FROM agent_task_queue WHERE issue_id = $1`, issueID)
	cleanupIssueCoordinationRows(t, issueID)

	eventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key":    "coordination-recovery-" + uuid.NewString(),
		"workspace_id": testWorkspaceID,
		"issue_id":     issueID,
		"event_type":   "task_completed",
		"status":       "pending",
		"payload":      testutil.Raw(`'{"assignment_role":"executor","agent_id":"` + executorID + `"}'::jsonb`),
	})
	assignmentID := dbfx.Insert(t, "agent_coordination_assignment", testutil.Cols{
		"event_id":     eventID,
		"workspace_id": testWorkspaceID,
		"issue_id":     issueID,
		"role":         "reviewer",
		"status":       "assigned",
		"owner_type":   "agent",
		"owner_id":     reviewerID,
		"decision": testutil.Raw(`'{
			"role":"reviewer",
			"review_publication":"review_handoff",
			"issue_update_publication_key":"review_handoff:recovery",
			"candidate_agent_id":"` + reviewerID + `",
			"explicit_reviewer":false,
			"previous_status":"in_progress",
			"previous_executor_type":"agent",
			"previous_executor_id":"` + executorID + `"
		}'::jsonb`),
	})
	taskID := dbfx.Task(t, reviewerID, testutil.Cols{
		"runtime_id": runtimeID,
		"issue_id":   issueID,
		"status":     "deferred",
		"context": testutil.Raw(`'{
			"coordination_assignment_id":"` + assignmentID + `",
			"coordination_assignment_role":"reviewer",
			"coordination_owner_type":"agent",
			"coordination_owner_id":"` + reviewerID + `"
		}'::jsonb`),
	})

	testHandler.AgentCoordination.RunOnce(context.Background())

	var eventStatus, assignmentStatus, taskStatus string
	var dispatchedTaskID *string
	dbfx.QueryRow(t, `SELECT status FROM agent_coordination_outbox WHERE id = $1`, eventID).Scan(&eventStatus)
	dbfx.QueryRow(t, `SELECT status, dispatched_task_id::text FROM agent_coordination_assignment WHERE id = $1`, assignmentID).Scan(&assignmentStatus, &dispatchedTaskID)
	dbfx.QueryRow(t, `SELECT status FROM agent_task_queue WHERE id = $1`, taskID).Scan(&taskStatus)
	if eventStatus != "completed" || assignmentStatus != "dispatched" || taskStatus != "queued" || dispatchedTaskID == nil || *dispatchedTaskID != taskID {
		var dispatchedTask string
		if dispatchedTaskID != nil {
			dispatchedTask = *dispatchedTaskID
		}
		t.Fatalf("recovered handoff = event %q assignment %q task %q dispatched %q; want completed/dispatched/queued/%q", eventStatus, assignmentStatus, taskStatus, dispatchedTask, taskID)
	}
}

func seedDispatchedReviewerCoordinationTask(t *testing.T, issueID, reviewerID string) string {
	t.Helper()
	eventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key":    "reviewer-task-fixture/" + uuid.NewString(),
		"workspace_id": testWorkspaceID,
		"issue_id":     issueID,
		"event_type":   "task_completed",
		"status":       "completed",
		"payload":      testutil.Raw("'{}'::jsonb"),
	})
	assignmentID := dbfx.Insert(t, "agent_coordination_assignment", testutil.Cols{
		"event_id":     eventID,
		"workspace_id": testWorkspaceID,
		"issue_id":     issueID,
		"role":         "reviewer",
		"status":       "dispatched",
		"owner_type":   "agent",
		"owner_id":     reviewerID,
	})
	taskID := dbfx.Task(t, reviewerID, testutil.Cols{
		"runtime_id": testRuntimeID,
		"issue_id":   issueID,
		"status":     "running",
	})
	dbfx.Exec(t, `UPDATE agent_coordination_assignment SET dispatched_task_id = $1 WHERE id = $2`, taskID, assignmentID)
	return taskID
}

func cleanupIssueCoordinationRows(t *testing.T, issueID string) {
	t.Helper()
	// Register outbox cleanup first so LIFO cleanup removes assignments before
	// their events. These broad deletes also cover rows produced by the handler.
	dbfx.Cleanup(t, `DELETE FROM agent_coordination_outbox WHERE issue_id = $1`, issueID)
	dbfx.Cleanup(t, `DELETE FROM agent_coordination_assignment WHERE issue_id = $1`, issueID)
}

func assertNoActiveIssueTasks(t *testing.T, issueID string) {
	t.Helper()
	var count int
	if err := testPool.QueryRow(context.Background(), `
		SELECT count(*)
		FROM agent_task_queue
		WHERE issue_id = $1
		  AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
	`, issueID).Scan(&count); err != nil {
		t.Fatalf("count active issue tasks: %v", err)
	}
	if count != 0 {
		t.Fatalf("active issue tasks = %d, want 0", count)
	}
}
