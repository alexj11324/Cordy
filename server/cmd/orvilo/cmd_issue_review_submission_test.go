package main

import (
	"reflect"
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
