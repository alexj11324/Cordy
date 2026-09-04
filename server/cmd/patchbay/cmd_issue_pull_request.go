package main

import (
	"context"
	"fmt"
	"net/url"
	"os"
	"strings"

	"github.com/patchbay-ai/patchbay/server/internal/cli"
	"github.com/spf13/cobra"
)

var issuePullRequestsCmd = &cobra.Command{
	Use:   "pull-requests <id>",
	Short: "List pull requests explicitly attached to an issue",
	Args:  exactArgs(1),
	RunE:  runIssuePullRequests,
}

var issuePullRequestCmd = &cobra.Command{
	Use:   "pull-request",
	Short: "Manage pull requests attached to an issue",
}

var issuePullRequestAttachCmd = &cobra.Command{
	Use:   "attach <id>",
	Short: "Explicitly attach a GitHub pull request to an issue",
	Args:  exactArgs(1),
	RunE:  runIssuePullRequestAttach,
}

func init() {
	issueCmd.AddCommand(issuePullRequestsCmd)
	issueCmd.AddCommand(issuePullRequestCmd)
	issuePullRequestCmd.AddCommand(issuePullRequestAttachCmd)
	issuePullRequestsCmd.Flags().String("output", "table", "Output format: table or json")
	issuePullRequestAttachCmd.Flags().String("url", "", "Canonical GitHub pull request URL (required)")
	issuePullRequestAttachCmd.Flags().String("title", "", "Optional display title fallback")
	issuePullRequestAttachCmd.Flags().String("state", "", "Optional state fallback: open, closed, merged, or draft")
	issuePullRequestAttachCmd.Flags().String("branch", "", "Optional head branch fallback")
	issuePullRequestAttachCmd.Flags().String("head-sha", "", "Optional head SHA fallback")
	issuePullRequestAttachCmd.Flags().Bool("close-intent", false, "Suggest closing the issue when this pull request is verified merged")
	issuePullRequestAttachCmd.Flags().String("output", "json", "Output format: table or json")
}

func runIssuePullRequests(cmd *cobra.Command, args []string) error {
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()
	issueRef, err := resolveIssueRef(ctx, client, args[0])
	if err != nil {
		return fmt.Errorf("resolve issue: %w", err)
	}
	var result map[string]any
	if err := client.GetJSON(ctx, "/api/issues/"+url.PathEscape(issueRef.ID)+"/pull-requests", &result); err != nil {
		return fmt.Errorf("list issue pull requests: %w", err)
	}
	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, result)
	}
	pullRequests, _ := result["pull_requests"].([]any)
	printIssuePullRequestsTable(pullRequests)
	return nil
}

func printIssuePullRequestsTable(raw []any) {
	rows := make([][]string, 0, len(raw))
	for _, item := range raw {
		pullRequest, ok := item.(map[string]any)
		if !ok {
			continue
		}
		pullRequestURL := strVal(pullRequest, "url")
		if pullRequestURL == "" {
			pullRequestURL = strVal(pullRequest, "html_url")
		}
		rows = append(rows, []string{
			strVal(pullRequest, "number"),
			strVal(pullRequest, "state"),
			strVal(pullRequest, "title"),
			pullRequestURL,
		})
	}
	cli.PrintTable(os.Stdout, []string{"NUMBER", "STATE", "TITLE", "URL"}, rows)
}

func runIssuePullRequestAttach(cmd *cobra.Command, args []string) error {
	requestURL, _ := cmd.Flags().GetString("url")
	requestURL = strings.TrimSpace(requestURL)
	if requestURL == "" {
		return fmt.Errorf("--url is required (https://github.com/{owner}/{repo}/pull/{number})")
	}
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()
	issueRef, err := resolveIssueRef(ctx, client, args[0])
	if err != nil {
		return fmt.Errorf("resolve issue: %w", err)
	}
	body := map[string]any{"url": requestURL}
	for flag, key := range map[string]string{
		"title": "title", "state": "state", "branch": "branch", "head-sha": "head_sha",
	} {
		value, _ := cmd.Flags().GetString(flag)
		if value = strings.TrimSpace(value); value != "" {
			body[key] = value
		}
	}
	closeIntent, _ := cmd.Flags().GetBool("close-intent")
	if closeIntent {
		body["close_intent"] = true
	}
	var result map[string]any
	if err := client.PostJSON(ctx, "/api/issues/"+url.PathEscape(issueRef.ID)+"/pull-requests", body, &result); err != nil {
		return fmt.Errorf("attach pull request: %w", err)
	}
	output, _ := cmd.Flags().GetString("output")
	pullRequest, _ := result["pull_request"].(map[string]any)
	if output == "table" {
		printIssuePullRequestsTable([]any{pullRequest})
		return nil
	}
	return cli.PrintJSON(os.Stdout, map[string]any{"pull_request": pullRequest})
}
