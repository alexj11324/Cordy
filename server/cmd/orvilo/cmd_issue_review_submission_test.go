package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

func TestIssueReviewSubmissionFlags(t *testing.T) {
	cmd := &cobra.Command{}
	addIssueReviewSubmissionFlags(cmd)
	if err := cmd.Flags().Set("review-worktree", "/work/project"); err != nil {
		t.Fatal(err)
	}
	if err := applyIssueReviewSubmissionFlags(cmd, map[string]any{}); err == nil {
		t.Fatal("partial handoff accepted")
	}
	for name, value := range map[string]string{"review-branch": "codex/review", "review-commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "review-pr": "https://github.com/example/project/pull/1,https://github.com/example/project/pull/2"} {
		if err := cmd.Flags().Set(name, value); err != nil {
			t.Fatal(err)
		}
	}
	body := map[string]any{}
	if err := applyIssueReviewSubmissionFlags(cmd, body); err != nil {
		t.Fatal(err)
	}
	packet := body["review_submission"].(map[string]any)
	if packet["worktree"] != "/work/project" || !reflect.DeepEqual(packet["pull_requests"], []string{"https://github.com/example/project/pull/1", "https://github.com/example/project/pull/2"}) {
		t.Fatalf("handoff=%#v", packet)
	}
}

func TestIssueStatusExposesReviewSubmissionFlags(t *testing.T) {
	for _, name := range []string{"review-worktree", "review-branch", "review-commit", "review-pr"} {
		if issueStatusCmd.Flags().Lookup(name) == nil {
			t.Errorf("issue status is missing --%s", name)
		}
	}
}

func TestRunIssueStatusSendsReviewSubmission(t *testing.T) {
	var body map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/api/issues/MUL-1":
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "issue-1", "identifier": "MUL-1", "status": "in_progress"})
		case r.Method == http.MethodPut && r.URL.Path == "/api/issues/issue-1":
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Errorf("decode status update: %v", err)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"id": "issue-1", "identifier": "MUL-1", "status": "in_review"})
		default:
			t.Errorf("unexpected request: %s %s", r.Method, r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	setCLITestServerEnv(t, srv.URL)

	cmd := newIssueStatusTestCmd()
	addIssueReviewSubmissionFlags(cmd)
	if err := cmd.ParseFlags([]string{
		"--review-worktree", "/work/project",
		"--review-branch", "codex/review",
		"--review-commit", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
		"--review-pr", "https://github.com/example/project/pull/1",
		"--review-pr", "https://github.com/example/project/pull/2",
		"--no-start",
	}); err != nil {
		t.Fatal(err)
	}
	if err := runIssueStatus(cmd, []string{"MUL-1", "in_review"}); err != nil {
		t.Fatalf("runIssueStatus: %v", err)
	}
	want := map[string]any{
		"status":       "in_review",
		"suppress_run": true,
		"review_submission": map[string]any{
			"worktree": "/work/project",
			"branch":   "codex/review",
			"commit":   "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
			"pull_requests": []any{
				"https://github.com/example/project/pull/1",
				"https://github.com/example/project/pull/2",
			},
		},
	}
	if !reflect.DeepEqual(body, want) {
		t.Fatalf("status update = %#v, want %#v", body, want)
	}
}

func TestRunIssueStatusRejectsPartialReviewSubmission(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		t.Errorf("partial handoff sent a request: %s %s", r.Method, r.URL.Path)
		http.NotFound(w, r)
	}))
	defer srv.Close()
	setCLITestServerEnv(t, srv.URL)

	cmd := newIssueStatusTestCmd()
	addIssueReviewSubmissionFlags(cmd)
	if err := cmd.Flags().Set("review-worktree", "/work/project"); err != nil {
		t.Fatal(err)
	}
	err := runIssueStatus(cmd, []string{"MUL-1", "in_review"})
	if err == nil || !strings.Contains(err.Error(), "review handoff requires") {
		t.Fatalf("error = %v, want incomplete review handoff error", err)
	}
}
