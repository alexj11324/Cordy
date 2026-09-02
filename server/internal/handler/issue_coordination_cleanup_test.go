package handler

import (
	"context"
	"testing"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestDeleteIssueCleansCoordinationRowsAndPreservesWorkspaceIsolation(t *testing.T) {
	requireIssueCoordinationDatabase(t)

	issueID := uuid.MustParse(createTestIssue(t, "issue coordination cleanup", "todo", "medium"))
	targetWorkspaceID := uuid.MustParse(testWorkspaceID)
	foreignWorkspaceID := uuid.New()

	seedIssueCoordinationRows(t, targetWorkspaceID, issueID, "pending")
	seedIssueCoordinationRows(t, foreignWorkspaceID, issueID, "pending")

	deleteTestIssue(t, issueID.String())

	if got := countIssuesForWorkspace(t, targetWorkspaceID, issueID); got != 0 {
		t.Fatalf("deleted issue count = %d, want 0", got)
	}
	assertIssueCoordinationCounts(t, targetWorkspaceID, issueID, 0, 0)
	assertIssueCoordinationCounts(t, foreignWorkspaceID, issueID, 1, 1)
	// The foreign rows are deliberately attached to the same issue UUID with a
	// different workspace fence. A bare issue_id cleanup would remove them.
}

func TestDeleteIssueLeavesNoClaimableCoordinationEvent(t *testing.T) {
	requireIssueCoordinationDatabase(t)

	issueID := uuid.MustParse(createTestIssue(t, "issue coordination worker cleanup", "todo", "medium"))
	targetWorkspaceID := uuid.MustParse(testWorkspaceID)
	seedIssueCoordinationRows(t, targetWorkspaceID, issueID, "processing")

	deleteTestIssue(t, issueID.String())
	assertIssueCoordinationCounts(t, targetWorkspaceID, issueID, 0, 0)

	claimed, err := db.New(testPool).ClaimAgentCoordinationOutbox(context.Background(), db.ClaimAgentCoordinationOutboxParams{
		LeaseOwner:   pgtype.Text{String: "issue-delete-test-worker", Valid: true},
		LeaseSeconds: 30,
		BatchSize:    10,
	})
	if err != nil {
		t.Fatalf("claim after issue delete: %v", err)
	}
	if len(claimed) != 0 {
		t.Fatalf("worker claimed %d events for a deleted issue, want 0", len(claimed))
	}
}

func requireIssueCoordinationDatabase(t *testing.T) {
	t.Helper()
	if testHandler == nil || testPool == nil {
		t.Skip("handler database fixture is unavailable")
	}
}

func seedIssueCoordinationRows(t *testing.T, workspaceID, issueID uuid.UUID, status string) {
	t.Helper()
	if status != "pending" && status != "processing" {
		t.Fatalf("unsupported coordination fixture status %q", status)
	}

	eventID := uuid.New()
	assignmentID := uuid.New()
	eventKey := "issue-delete-test/" + eventID.String()
	ctx := context.Background()
	_, err := testPool.Exec(ctx, `
		INSERT INTO agent_coordination_outbox (
			id, event_key, workspace_id, issue_id, event_type, status,
			lease_owner, lease_expires_at, payload
		)
		VALUES (
			$1, $2, $3, $4, 'task_completed', $5,
			CASE WHEN $5::text = 'processing' THEN 'old-worker' ELSE NULL END,
			CASE WHEN $5::text = 'processing' THEN now() - interval '1 second' ELSE NULL END,
			'{}'::jsonb
		)
	`, eventID, eventKey, workspaceID, issueID, status)
	if err != nil {
		t.Fatalf("seed coordination outbox: %v", err)
	}
	_, err = testPool.Exec(ctx, `
		INSERT INTO agent_coordination_assignment (
			id, event_id, workspace_id, issue_id, role, status
		)
		VALUES ($1, $2, $3, $4, 'executor', 'pending')
	`, assignmentID, eventID, workspaceID, issueID)
	if err != nil {
		t.Fatalf("seed coordination assignment: %v", err)
	}

	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), `DELETE FROM agent_coordination_assignment WHERE id = $1`, assignmentID)
		_, _ = testPool.Exec(context.Background(), `DELETE FROM agent_coordination_outbox WHERE id = $1`, eventID)
	})
}

func assertIssueCoordinationCounts(t *testing.T, workspaceID, issueID uuid.UUID, wantOutbox, wantAssignments int64) {
	t.Helper()
	var outbox, assignments int64
	workspace := pgtype.UUID{Bytes: workspaceID, Valid: true}
	issue := pgtype.UUID{Bytes: issueID, Valid: true}
	err := testPool.QueryRow(context.Background(), `
		SELECT
			(SELECT count(*) FROM agent_coordination_outbox WHERE workspace_id = $1 AND issue_id = $2),
			(SELECT count(*) FROM agent_coordination_assignment WHERE workspace_id = $1 AND issue_id = $2)
	`, workspace, issue).Scan(&outbox, &assignments)
	if err != nil {
		t.Fatalf("count issue coordination rows: %v", err)
	}
	if outbox != wantOutbox || assignments != wantAssignments {
		t.Fatalf("coordination rows for workspace %s issue %s = outbox %d, assignments %d; want %d, %d", workspaceID, issueID, outbox, assignments, wantOutbox, wantAssignments)
	}
}

func countIssuesForWorkspace(t *testing.T, workspaceID, issueID uuid.UUID) int64 {
	t.Helper()
	var count int64
	err := testPool.QueryRow(context.Background(), `
		SELECT count(*) FROM issue WHERE workspace_id = $1 AND id = $2
	`, pgtype.UUID{Bytes: workspaceID, Valid: true}, pgtype.UUID{Bytes: issueID, Valid: true}).Scan(&count)
	if err != nil {
		t.Fatalf("count deleted issue: %v", err)
	}
	return count
}
