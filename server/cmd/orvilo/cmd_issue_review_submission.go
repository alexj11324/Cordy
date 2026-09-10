package main

import (
	"errors"
	"strings"

	"github.com/spf13/cobra"
)

func addIssueReviewSubmissionFlags(cmd *cobra.Command) {
	cmd.Flags().String("review-worktree", "", "Worktree path handed off for review")
	cmd.Flags().String("review-branch", "", "Branch handed off for review")
	cmd.Flags().String("review-commit", "", "Full commit SHA handed off for review")
	cmd.Flags().StringSlice("review-pr", nil, "PR URL handed off for review; repeat for multiple PRs")
}

func applyIssueReviewSubmissionFlags(cmd *cobra.Command, body map[string]any) error {
	if cmd.Flags().Lookup("review-worktree") == nil {
		return nil
	}
	changed := false
	for _, name := range []string{"review-worktree", "review-branch", "review-commit", "review-pr"} {
		changed = changed || cmd.Flags().Changed(name)
	}
	if !changed {
		return nil
	}
	worktree, _ := cmd.Flags().GetString("review-worktree")
	branch, _ := cmd.Flags().GetString("review-branch")
	commit, _ := cmd.Flags().GetString("review-commit")
	prs, _ := cmd.Flags().GetStringSlice("review-pr")
	if strings.TrimSpace(worktree) == "" || strings.TrimSpace(branch) == "" || strings.TrimSpace(commit) == "" || len(prs) == 0 {
		return errors.New("review handoff requires --review-worktree, --review-branch, --review-commit, and --review-pr together")
	}
	body["review_submission"] = map[string]any{"worktree": worktree, "branch": branch, "commit": commit, "pull_requests": prs}
	return nil
}
