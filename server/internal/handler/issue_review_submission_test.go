package handler

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgconn"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func reviewSubmissionFixture() map[string]any {
	return map[string]any{"worktree": "/work/project", "branch": "codex/review", "commit": strings.Repeat("a", 40), "pull_requests": []string{"https://github.com/example/project/pull/42"}, "submission_id": "fixture-submission"}
}

func reviewSubmissionDBFixture() []byte {
	encoded, _ := json.Marshal(reviewSubmissionFixture())
	return encoded
}

func TestIssueReviewSubmissionRequiresCompleteAtomicHandoff(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Review submission executor", nil)
	issueID := dbfx.Issue(t, "Review submission fixture")
	dbfx.Exec(t, `UPDATE issue SET status='in_progress', executor_type='agent', executor_id=$2 WHERE id=$1`, issueID, agentID)
	update := func(submission any) *httptest.ResponseRecorder {
		body := map[string]any{"status": "in_review", "reviewer_type": "member", "reviewer_id": testUserID, "suppress_run": true}
		if submission != nil {
			body["review_submission"] = submission
		}
		w := httptest.NewRecorder()
		testHandler.UpdateIssue(w, withURLParam(newRequest(http.MethodPatch, "/api/issues/"+issueID, body), "id", issueID))
		return w
	}
	for _, missing := range []string{"all", "worktree", "branch", "commit", "pull_requests"} {
		t.Run(missing, func(t *testing.T) {
			var submission any
			if missing != "all" {
				partial := reviewSubmissionFixture()
				delete(partial, missing)
				submission = partial
			}
			w := update(submission)
			if w.Code != http.StatusBadRequest {
				t.Fatalf("missing %s: status=%d body=%s", missing, w.Code, w.Body.String())
			}
			issue, err := testHandler.Queries.GetIssue(context.Background(), parseUUID(issueID))
			if err != nil {
				t.Fatal(err)
			}
			if issue.Status != "in_progress" || issue.ReviewerID.Valid || len(issue.ReviewSubmission) > 0 {
				t.Fatalf("failed review partially committed: status=%s reviewer=%v metadata=%s", issue.Status, issue.ReviewerID.Valid, issue.Metadata)
			}
		})
	}
	w := update(reviewSubmissionFixture())
	if w.Code != http.StatusOK {
		t.Fatalf("complete handoff: status=%d body=%s", w.Code, w.Body.String())
	}
	var response IssueResponse
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	if response.Status != "in_review" {
		t.Fatalf("status=%s", response.Status)
	}
	var packet map[string]any
	err := json.Unmarshal(response.ReviewSubmission, &packet)
	ok := err == nil
	if !ok || packet["submitted_at"] == nil || packet["submission_id"] == "" {
		t.Fatalf("missing persisted handoff: %#v", response.ReviewSubmission)
	}
	changed := reviewSubmissionFixture()
	changed["commit"] = strings.Repeat("b", 40)
	if result := update(changed); result.Code != http.StatusBadRequest {
		t.Fatalf("active review packet replaced: status=%d body=%s", result.Code, result.Body.String())
	}
	persisted, err := testHandler.Queries.GetIssue(context.Background(), parseUUID(issueID))
	if err != nil {
		t.Fatal(err)
	}
	var stored map[string]any
	if err := json.Unmarshal(persisted.ReviewSubmission, &stored); err != nil {
		t.Fatal(err)
	}
	if stored["commit"] != strings.Repeat("a", 40) {
		t.Fatalf("active handoff changed: %s", persisted.ReviewSubmission)
	}
}

func TestIssueReviewSubmissionDatabaseGateCoversCustomStatusAndStaleHandoff(t *testing.T) {
	custom := createTestCustomStatus(t, "review_packet_gate", "in_review")
	issueID := dbfx.Issue(t, "Review SQL gate")
	agentID := createHandlerTestAgent(t, "SQL review executor", nil)
	dbfx.Exec(t, `UPDATE issue SET executor_type='agent', executor_id=$2, reviewer_type='member', reviewer_id=$3 WHERE id=$1`, issueID, agentID, testUserID)
	_, err := testHandler.DB.Exec(context.Background(), `UPDATE issue SET status=$2 WHERE id=$1`, issueID, custom.Key)
	var pgErr *pgconn.PgError
	if !errors.As(err, &pgErr) || pgErr.ConstraintName != "issue_review_submission_required" {
		t.Fatalf("missing DB guard: %v", err)
	}
	encoded, err := encodeIssueReviewSubmission(&issueReviewSubmission{Worktree: "/work/project", Branch: "codex/review", Commit: strings.Repeat("a", 40), PullRequests: []string{"https://github.com/example/project/pull/42"}})
	if err != nil {
		t.Fatal(err)
	}
	for _, reviewerID := range []any{nil, agentID} {
		_, err = testHandler.DB.Exec(context.Background(), `UPDATE issue SET status=$2, review_submission=$3::jsonb, reviewer_type='agent', reviewer_id=$4 WHERE id=$1`, issueID, custom.Key, encoded, reviewerID)
		if !errors.As(err, &pgErr) || pgErr.ConstraintName != "issue_review_reviewer_required" {
			t.Fatalf("invalid reviewer accepted by DB: %v", err)
		}
	}
	dbfx.Exec(t, `UPDATE issue SET status=$2, review_submission=$3::jsonb WHERE id=$1`, issueID, custom.Key, encoded)
	_, err = testHandler.DB.Exec(context.Background(), `UPDATE issue SET reviewer_type=NULL, reviewer_id=NULL WHERE id=$1`, issueID)
	if !errors.As(err, &pgErr) || pgErr.ConstraintName != "issue_review_reviewer_required" {
		t.Fatalf("active reviewer removed through DB: %v", err)
	}
	dbfx.Exec(t, `UPDATE issue SET status='in_progress' WHERE id=$1`, issueID)
	_, err = testHandler.DB.Exec(context.Background(), `UPDATE issue SET status=$2 WHERE id=$1`, issueID, custom.Key)
	if !errors.As(err, &pgErr) || pgErr.ConstraintName != "issue_review_submission_required" {
		t.Fatalf("stale handoff reused: %v", err)
	}
	got, err := testHandler.Queries.GetIssueInWorkspace(context.Background(), db.GetIssueInWorkspaceParams{ID: parseUUID(issueID), WorkspaceID: parseUUID(testWorkspaceID)})
	if err != nil || got.Status != "in_progress" {
		t.Fatalf("invalid transition committed: %s %v", got.Status, err)
	}
}

func TestIssueReviewSubmissionCreateRejectsMissingEvidence(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Review create executor", nil)
	testutil.Decode[map[string]any](t, testHandler.CreateIssue, newRequest(http.MethodPost, "/api/issues?workspace_id="+testWorkspaceID,
		map[string]any{"title": "No handoff review create", "status": "in_review", "executor_type": "agent", "executor_id": agentID, "reviewer_type": "member", "reviewer_id": testUserID}), http.StatusBadRequest)
}

func TestIssueReviewSubmissionMoveAcceptsCompleteHandoff(t *testing.T) {
	agentID := createHandlerTestAgent(t, "Move review executor", nil)
	issueID := dbfx.Issue(t, "Move review packet")
	dbfx.Exec(t, `UPDATE issue SET status='in_progress', executor_type='agent', executor_id=$2 WHERE id=$1`, issueID, agentID)
	got := testutil.Decode[IssueResponse](t, testHandler.MoveIssue,
		withURLParam(newRequest(http.MethodPost, "/api/issues/"+issueID+"/move", map[string]any{
			"status": "in_review", "reviewer_type": "member", "reviewer_id": testUserID,
			"before_id": nil, "after_id": nil, "review_submission": reviewSubmissionFixture(),
		}), "id", issueID), http.StatusOK)
	if got.Status != "in_review" || len(got.ReviewSubmission) == 0 {
		t.Fatalf("move lost handoff: %+v", got)
	}
}
