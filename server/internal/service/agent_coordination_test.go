package service

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestDecodeCoordinationTaskContext(t *testing.T) {
	tests := []struct {
		name    string
		input   string
		present bool
		wantID  string
	}{
		{name: "empty", input: ``, present: false},
		{name: "ordinary task context", input: `{"head_sha":"abc"}`, present: false},
		{name: "coordination context", input: `{"coordination_assignment_id":"00000000-0000-0000-0000-000000000001","coordination_assignment_role":"executor"}`, present: true, wantID: "00000000-0000-0000-0000-000000000001"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, present, err := decodeCoordinationTaskContext([]byte(tt.input))
			if err != nil {
				t.Fatalf("decode context: %v", err)
			}
			if present != tt.present {
				t.Fatalf("present = %v, want %v", present, tt.present)
			}
			if got.AssignmentID != tt.wantID {
				t.Fatalf("assignment id = %q, want %q", got.AssignmentID, tt.wantID)
			}
		})
	}
}

func TestCoordinationTaskContextPreservesFences(t *testing.T) {
	assignment := db.AgentCoordinationAssignment{
		ID:        testCoordinationUUID("00000000-0000-0000-0000-000000000001"),
		Role:      CoordinationAssignmentExecutor,
		OwnerType: pgtype.Text{String: "agent", Valid: true},
		OwnerID:   testCoordinationUUID("00000000-0000-0000-0000-000000000002"),
	}
	issue := db.Issue{
		ID:                 testCoordinationUUID("00000000-0000-0000-0000-000000000003"),
		WorkspaceID:        testCoordinationUUID("00000000-0000-0000-0000-000000000004"),
		Revision:           17,
		ExecutorType:       pgtype.Text{String: "agent", Valid: true},
		ExecutorID:         assignment.OwnerID,
		ExecutorGeneration: 9,
	}
	raw, err := marshalCoordinationTaskContext(assignment, issue)
	if err != nil {
		t.Fatalf("marshal context: %v", err)
	}
	var fields map[string]any
	if err := json.Unmarshal(raw, &fields); err != nil {
		t.Fatalf("unmarshal context: %v", err)
	}
	if fields[coordinationAssignmentIDContextKey] != util.UUIDToString(assignment.ID) {
		t.Fatalf("assignment fence missing: %#v", fields)
	}
	if fields[coordinationOwnerGenerationKey] != float64(9) {
		t.Fatalf("owner generation = %#v, want 9", fields[coordinationOwnerGenerationKey])
	}
	if fields[coordinationIssueRevisionKey] != float64(17) {
		t.Fatalf("issue revision = %#v, want 17", fields[coordinationIssueRevisionKey])
	}
}

func TestCoordinationTaskEligibilityExcludesChatAndSideChat(t *testing.T) {
	issueTask := db.AgentTaskQueue{IssueID: testCoordinationUUID("00000000-0000-0000-0000-000000000001")}
	if !coordinationTaskEligible(issueTask) {
		t.Fatal("plain issue task should be eligible")
	}
	issueTask.ChatSessionID = testCoordinationUUID("00000000-0000-0000-0000-000000000002")
	if coordinationTaskEligible(issueTask) {
		t.Fatal("chat-linked task should not be eligible")
	}
	issueTask.ChatSessionID = pgtype.UUID{}
	issueTask.Context = []byte(`{"side_chat_parent_task_id":"00000000-0000-0000-0000-000000000003"}`)
	if coordinationTaskEligible(issueTask) {
		t.Fatal("side-chat task should not be eligible")
	}
	issueTask.Context = []byte(`{"side_chat_root_comment_id":"00000000-0000-0000-0000-000000000004"}`)
	if coordinationTaskEligible(issueTask) {
		t.Fatal("root-comment side-chat task should not be eligible")
	}
}

func TestCoordinationCompletionRejectsSupersededGeneration(t *testing.T) {
	agentID := testCoordinationUUID("00000000-0000-0000-0000-000000000002")
	issue := db.Issue{
		ExecutorType:       pgtype.Text{String: "agent", Valid: true},
		ExecutorID:         agentID,
		ExecutorGeneration: 4,
	}
	oldGeneration := int64(3)
	context := coordinationTaskContext{OwnerID: util.UUIDToString(agentID), OwnerGeneration: &oldGeneration}
	if coordinationCompletionStillOwnsIssue(issue, CoordinationAssignmentExecutor, context, agentID) {
		t.Fatal("stale executor generation should be rejected")
	}
	context.OwnerGeneration = int64Ptr(4)
	if !coordinationCompletionStillOwnsIssue(issue, CoordinationAssignmentExecutor, context, agentID) {
		t.Fatal("current executor generation should be accepted")
	}
}

func TestCoordinationTaskCompletionRequiresAssignmentIdentity(t *testing.T) {
	assignmentID := testCoordinationUUID("00000000-0000-0000-0000-000000000001")
	agentID := testCoordinationUUID("00000000-0000-0000-0000-000000000002")
	taskID := testCoordinationUUID("00000000-0000-0000-0000-000000000003")
	assignment := db.AgentCoordinationAssignment{
		ID:               assignmentID,
		Role:             CoordinationAssignmentReviewer,
		Status:           "dispatched",
		OwnerType:        pgtype.Text{String: "agent", Valid: true},
		OwnerID:          agentID,
		DispatchedTaskID: taskID,
	}
	task := db.AgentTaskQueue{ID: taskID, AgentID: agentID}
	context := coordinationTaskContext{
		AssignmentID:   util.UUIDToString(assignmentID),
		AssignmentRole: CoordinationAssignmentReviewer,
	}
	if !coordinationAssignmentMatchesTask(assignment, task, context) {
		t.Fatal("matching assignment identity should be accepted")
	}
	context.AssignmentID = "00000000-0000-0000-0000-000000000004"
	if coordinationAssignmentMatchesTask(assignment, task, context) {
		t.Fatal("different assignment identity should be rejected")
	}
}

func TestCoordinationOwnerMatchesCurrentIssueAssignment(t *testing.T) {
	ownerID := testCoordinationUUID("00000000-0000-0000-0000-000000000002")
	issue := db.Issue{
		ExecutorType:       pgtype.Text{String: "agent", Valid: true},
		ExecutorID:         ownerID,
		ExecutorGeneration: 4,
	}
	if !coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "agent", ownerID, nil) {
		t.Fatal("current agent executor should be eligible")
	}
	if coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "member", ownerID, nil) {
		t.Fatal("member owner must not be dispatched as an agent")
	}
	otherID := testCoordinationUUID("00000000-0000-0000-0000-000000000003")
	if coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "agent", otherID, nil) {
		t.Fatal("stale agent owner should not be dispatched")
	}
	oldGeneration := int64(3)
	if coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "agent", ownerID, &oldGeneration) {
		t.Fatal("stale executor generation should not be dispatched")
	}
	currentGeneration := int64(4)
	if !coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "agent", ownerID, &currentGeneration) {
		t.Fatal("current executor generation should be dispatched")
	}
}

func TestCoordinationTeamOwnerUsesTeamProvenanceAndLeaderTask(t *testing.T) {
	teamID := testCoordinationUUID("00000000-0000-0000-0000-000000000010")
	leaderID := testCoordinationUUID("00000000-0000-0000-0000-000000000011")
	issue := db.Issue{
		ExecutorType:       pgtype.Text{String: "team", Valid: true},
		ExecutorID:         teamID,
		ExecutorGeneration: 6,
	}
	if !coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "team", teamID, int64Ptr(6)) {
		t.Fatal("current team executor should be dispatchable through its leader")
	}
	if coordinationOwnerMatchesIssue(issue, CoordinationAssignmentExecutor, "agent", leaderID, int64Ptr(6)) {
		t.Fatal("leader agent must not replace the durable team owner")
	}

	assignment := db.AgentCoordinationAssignment{
		ID:               testCoordinationUUID("00000000-0000-0000-0000-000000000012"),
		Role:             CoordinationAssignmentExecutor,
		Status:           "dispatched",
		OwnerType:        pgtype.Text{String: "team", Valid: true},
		OwnerID:          teamID,
		DispatchedTaskID: testCoordinationUUID("00000000-0000-0000-0000-000000000013"),
	}
	task := db.AgentTaskQueue{ID: assignment.DispatchedTaskID, AgentID: leaderID}
	context := coordinationTaskContext{
		AssignmentID:     util.UUIDToString(assignment.ID),
		AssignmentRole:   CoordinationAssignmentExecutor,
		OwnerType:        "team",
		OwnerID:          util.UUIDToString(teamID),
		OwnerGeneration: int64Ptr(6),
	}
	if !coordinationAssignmentMatchesTask(assignment, task, context) {
		t.Fatal("team assignment should accept the team's leader task")
	}
	if !coordinationCompletionStillOwnsIssue(issue, CoordinationAssignmentExecutor, context, leaderID) {
		t.Fatal("team completion should be fenced by the team while executed by its leader")
	}
	issue.ExecutorGeneration = 7
	if coordinationCompletionStillOwnsIssue(issue, CoordinationAssignmentExecutor, context, leaderID) {
		t.Fatal("team completion with a stale generation should be rejected")
	}
}

func TestCoordinationFollowUpUsesExplicitFalse(t *testing.T) {
	if !coordinationFollowUp(coordinationEventPayload{}) {
		t.Fatal("legacy/missing follow_up should remain retryable")
	}
	falseValue := false
	if coordinationFollowUp(coordinationEventPayload{FollowUp: &falseValue}) {
		t.Fatal("explicit follow_up=false should not dispatch a child")
	}
}

func TestCoordinationEventAssignmentRoleAllowsImplementationToReviewerHandoff(t *testing.T) {
	event := db.AgentCoordinationOutbox{EventType: CoordinationEventTaskCompleted}
	payload := coordinationEventPayload{AssignmentRole: CoordinationAssignmentExecutor}
	assignment := db.AgentCoordinationAssignment{Role: CoordinationAssignmentReviewer}
	if !coordinationEventAssignmentRoleMatches(event, payload, assignment) {
		t.Fatal("implementation completion should be allowed to target reviewer assignment")
	}

	payload.AssignmentRole = CoordinationAssignmentReviewer
	if !coordinationEventAssignmentRoleMatches(event, payload, assignment) {
		t.Fatal("reviewer event should match reviewer assignment")
	}

	event.EventType = CoordinationEventReviewReturned
	payload.AssignmentRole = CoordinationAssignmentExecutor
	if coordinationEventAssignmentRoleMatches(event, payload, assignment) {
		t.Fatal("review return must not target reviewer assignment")
	}
}

func TestCoordinationHandoffNoteHasAUsefulDefault(t *testing.T) {
	if got := coordinationHandoffNote("", "in_review"); got == "" || !strings.Contains(got, "implementation task completed") {
		t.Fatalf("review handoff default = %q", got)
	}
	if got := coordinationHandoffNote("", "in_progress"); got == "" || !strings.Contains(got, "review feedback") {
		t.Fatalf("review return default = %q", got)
	}
	if got := coordinationHandoffNote("  explicit note  ", "in_review"); got != "explicit note" {
		t.Fatalf("explicit handoff note = %q", got)
	}
}

func TestCoordinationDispatchOwnerRequiresExplicitAgentType(t *testing.T) {
	ownerID := testCoordinationUUID("00000000-0000-0000-0000-000000000002")
	event := db.AgentCoordinationOutbox{EventType: CoordinationEventTaskCompleted}
	assignment := db.AgentCoordinationAssignment{OwnerID: ownerID}
	ownerType, gotID, err := coordinationDispatchOwner(event, coordinationEventPayload{}, assignment, db.Issue{})
	if err != nil {
		t.Fatalf("resolve owner: %v", err)
	}
	if ownerType != "" || !sameCoordinationUUID(gotID, ownerID) {
		t.Fatalf("owner = (%q, %v), want empty type with preserved id", ownerType, gotID)
	}
}

func TestCoordinationEventTypesMatchSchemaContract(t *testing.T) {
	if CoordinationEventTaskCompleted != "task_completed" {
		t.Fatalf("task completion event changed: %q", CoordinationEventTaskCompleted)
	}
	if CoordinationEventReviewReturned != "review_returned" {
		t.Fatalf("review return event changed: %q", CoordinationEventReviewReturned)
	}
	if CoordinationEventTaskCompleted == CoordinationEventReviewReturned {
		t.Fatal("coordination event contracts must remain distinguishable")
	}
}

func testCoordinationUUID(value string) pgtype.UUID {
	return util.MustParseUUID(value)
}
