package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
)

func TestUpdateIssue_ReviewReturnRetiresReviewerTaskAndRecordsExecutorHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)

	executorID := dbfx.Agent(t, "review-return-executor", testRuntimeID)
	reviewerID := dbfx.Agent(t, "review-return-reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "review return coordination", testutil.Cols{
		"status":        "in_review",
		"executor_type": "agent",
		"executor_id":   executorID,
		"reviewer_type": "agent",
		"reviewer_id":   reviewerID,
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

	executorID := dbfx.Agent(t, "reviewer-reassign-executor", testRuntimeID)
	previousReviewerID := dbfx.Agent(t, "previous-reviewer", testRuntimeID)
	nextReviewerID := dbfx.Agent(t, "next-reviewer", testRuntimeID)
	issueID := dbfx.Issue(t, "reviewer reassignment coordination", testutil.Cols{
		"status":        "in_review",
		"executor_type": "agent",
		"executor_id":   executorID,
		"reviewer_type": "agent",
		"reviewer_id":   previousReviewerID,
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
