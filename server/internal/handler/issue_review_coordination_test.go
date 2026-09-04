package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

func TestUpdateIssue_ReviewReturnRetiresReviewerTaskAndRecordsExecutorHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })

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
	disableIssueRoleDefaults = true
	t.Cleanup(func() { disableIssueRoleDefaults = false })

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

func TestAgentCoordinationRunOnceSelectsReviewerAndPublishesHandoff(t *testing.T) {
	requireIssueCoordinationDatabase(t)

	executorID := dbfx.Agent(t, "coordination implementation", testRuntimeID)
	reviewerID := dbfx.Agent(t, "coordination reviewer", testRuntimeID)
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
		"runtime_id":   testRuntimeID,
		"issue_id":     issueID,
		"status":       "completed",
		"completed_at": testutil.Raw("now()"),
		"context":      testutil.Raw("'{}'::jsonb"),
	})
	sourceEventID := dbfx.Insert(t, "agent_coordination_outbox", testutil.Cols{
		"event_key":    "coordination-source-" + uuid.NewString(),
		"workspace_id": testWorkspaceID,
		"issue_id":     issueID,
		"source_task_id": nil,
		"event_type":   "task_completed",
		"status":       "completed",
		"payload":      testutil.Raw("'{}'::jsonb"),
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
			'coordination_assignment_id', $1,
			'coordination_assignment_role', 'executor',
			'coordination_owner_type', 'agent',
			'coordination_owner_id', $2
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
