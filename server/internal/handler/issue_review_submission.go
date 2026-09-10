package handler

import (
	"encoding/json"
	"errors"
	"net/http"
	"net/url"
	"regexp"
	"strings"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgconn"
)

type issueReviewSubmission struct {
	Worktree     string   `json:"worktree"`
	Branch       string   `json:"branch"`
	Commit       string   `json:"commit"`
	PullRequests []string `json:"pull_requests"`
}

var reviewCommitPattern = regexp.MustCompile(`^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$`)
var reviewPRPathPattern = regexp.MustCompile(`/(pull|pulls|merge_requests)/[0-9]+/?$`)

func encodeIssueReviewSubmission(input *issueReviewSubmission) ([]byte, error) {
	if input == nil {
		return nil, nil
	}
	input.Worktree = strings.TrimSpace(input.Worktree)
	input.Branch = strings.TrimSpace(input.Branch)
	input.Commit = strings.TrimSpace(input.Commit)
	if input.Worktree == "" || input.Branch == "" || !reviewCommitPattern.MatchString(input.Commit) || len(input.PullRequests) == 0 {
		return nil, errors.New("review requires worktree, branch, full commit SHA, and at least one PR")
	}
	for i, raw := range input.PullRequests {
		u, err := url.Parse(strings.TrimSpace(raw))
		if err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Hostname() == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" || !reviewPRPathPattern.MatchString(u.Path) {
			return nil, errors.New("review requires valid PR URLs")
		}
		input.PullRequests[i] = u.String()
	}
	return json.Marshal(struct {
		*issueReviewSubmission
		SubmissionID string `json:"submission_id"`
	}{input, uuid.NewString()})
}

func writeReviewSubmissionError(w http.ResponseWriter, err error) bool {
	var pgErr *pgconn.PgError
	if !errors.As(err, &pgErr) {
		return false
	}
	if pgErr.ConstraintName == "issue_review_submission_immutable" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"code": "review_submission_immutable", "error": "Return to In Progress before replacing the active review handoff."})
		return true
	}
	if pgErr.ConstraintName == "issue_review_reviewer_required" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"code": "review_reviewer_required", "error": "Select a reviewer different from the executor before entering review."})
		return true
	}
	if pgErr.ConstraintName != "issue_review_submission_required" {
		return false
	}
	writeJSON(w, http.StatusBadRequest, map[string]string{
		"code":  "review_submission_required",
		"error": "Submit worktree, PR, branch, and full commit SHA together before entering review.",
	})
	return true
}
