package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/url"
	"os"
	"strconv"
	"strings"

	"github.com/spf13/cobra"

	"github.com/patchbay-ai/patchbay/server/internal/cli"
)

// `patchbay issue dependency-graph` is the CLI surface for the typed graph
// contract. The server remains the authority for role validation, topology,
// wave derivation, and atomic persistence; the CLI only reads a complete plan
// and forwards it with its idempotency key.
var issueDependencyGraphCmd = &cobra.Command{
	Use:   "dependency-graph",
	Short: "Inspect or atomically apply a dependency graph for an issue",
}

var issueDependencyGraphGetCmd = &cobra.Command{
	Use:   "get <id>",
	Short: "Get the persisted dependency graph for an issue",
	Args:  exactArgs(1),
	RunE:  runIssueDependencyGraphGet,
}

var issueDependencyGraphApplyCmd = &cobra.Command{
	Use:   "apply <parent-id>",
	Short: "Validate and atomically apply a typed dependency plan",
	Args:  exactArgs(1),
	RunE:  runIssueDependencyGraphApply,
}

func init() {
	issueDependencyGraphCmd.AddCommand(issueDependencyGraphGetCmd)
	issueDependencyGraphCmd.AddCommand(issueDependencyGraphApplyCmd)

	issueDependencyGraphGetCmd.Flags().String("output", "json", "Output format: table or json")

	issueDependencyGraphApplyCmd.Flags().String("idempotency-key", "", "Plan idempotency key; reuse it to safely replay the same plan")
	issueDependencyGraphApplyCmd.Flags().String("plan-file", "", "Read the complete typed plan from a UTF-8 JSON file")
	issueDependencyGraphApplyCmd.Flags().Bool("plan-stdin", false, "Read the complete typed plan from stdin as UTF-8 JSON")
	issueDependencyGraphApplyCmd.Flags().Bool("allow-external-file", false, "Allow --plan-file outside the current working directory")
	issueDependencyGraphApplyCmd.Flags().String("output", "json", "Output format: table or json")

	issueCmd.AddCommand(issueDependencyGraphCmd)
}

func runIssueDependencyGraphGet(cmd *cobra.Command, args []string) error {
	output, err := dependencyGraphOutputFormat(cmd)
	if err != nil {
		return err
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
	var graph map[string]any
	if err := client.GetJSON(ctx, "/api/issues/"+url.PathEscape(issueRef.ID)+"/dependency-graph", &graph); err != nil {
		return fmt.Errorf("get dependency graph: %w", err)
	}
	return printDependencyGraphResult(output, graph)
}

func runIssueDependencyGraphApply(cmd *cobra.Command, args []string) error {
	output, err := dependencyGraphOutputFormat(cmd)
	if err != nil {
		return err
	}
	idempotencyKey, _ := cmd.Flags().GetString("idempotency-key")
	idempotencyKey = strings.TrimSpace(idempotencyKey)
	if idempotencyKey == "" {
		return fmt.Errorf("--idempotency-key must not be empty")
	}

	plan, err := readDependencyGraphPlan(cmd)
	if err != nil {
		return err
	}
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	parentRef, err := resolveIssueRef(ctx, client, args[0])
	if err != nil {
		return fmt.Errorf("resolve parent issue: %w", err)
	}
	var graph map[string]any
	if err := client.PostJSONWithHeader(
		ctx,
		"/api/issues/"+url.PathEscape(parentRef.ID)+"/dependency-graph/apply",
		plan,
		"Idempotency-Key",
		idempotencyKey,
		&graph,
	); err != nil {
		return fmt.Errorf("atomically apply dependency graph: %w", err)
	}
	return printDependencyGraphResult(output, graph)
}

func dependencyGraphOutputFormat(cmd *cobra.Command) (string, error) {
	output, _ := cmd.Flags().GetString("output")
	switch output {
	case "json", "table":
		return output, nil
	default:
		return "", fmt.Errorf("--output must be table or json, got %q", output)
	}
}

func readDependencyGraphPlan(cmd *cobra.Command) (map[string]any, error) {
	planFile, _ := cmd.Flags().GetString("plan-file")
	planStdin, _ := cmd.Flags().GetBool("plan-stdin")
	if planFile != "" && planStdin {
		return nil, fmt.Errorf("--plan-file and --plan-stdin are mutually exclusive")
	}
	if planFile == "" && !planStdin {
		return nil, fmt.Errorf("one of --plan-file or --plan-stdin is required")
	}

	var raw []byte
	if planStdin {
		var err error
		raw, err = io.ReadAll(cmd.InOrStdin())
		if err != nil {
			return nil, fmt.Errorf("read typed dependency plan from stdin: %w", err)
		}
	} else {
		if err := ensureFileFlagWithinWorkdir(cmd, "plan-file", "plan", planFile); err != nil {
			return nil, err
		}
		var err error
		raw, err = os.ReadFile(planFile)
		if err != nil {
			return nil, fmt.Errorf("read dependency plan %q: %w", planFile, err)
		}
	}
	if strings.TrimSpace(string(raw)) == "" {
		return nil, fmt.Errorf("typed dependency plan is empty")
	}
	var plan map[string]any
	if err := json.Unmarshal(raw, &plan); err != nil {
		return nil, fmt.Errorf("parse typed dependency plan JSON: %w", err)
	}
	if plan == nil {
		return nil, fmt.Errorf("typed dependency plan must be a JSON object")
	}
	return plan, nil
}

func printDependencyGraphResult(output string, graph map[string]any) error {
	if output == "json" {
		return cli.PrintJSON(os.Stdout, graph)
	}
	cli.PrintTable(os.Stdout, dependencyGraphTableHeaders(), dependencyGraphTableRows(graph))
	return nil
}

func dependencyGraphTableHeaders() []string {
	return []string{"FIELD", "VALUE"}
}

func dependencyGraphTableRows(graph map[string]any) [][]string {
	plan, _ := graph["plan"].(map[string]any)
	nodes, _ := graph["nodes"].([]any)
	if len(nodes) == 0 {
		nodes, _ = graph["children"].([]any)
	}

	rows := [][]string{
		{"PLAN", strVal(plan, "id")},
		{"GOAL", strVal(plan, "goal")},
		{"STATUS", strVal(plan, "status")},
	}
	if attention := strVal(plan, "attention_reason"); attention != "" {
		rows = append(rows, []string{"ATTENTION", attention})
	}
	ready, blocked := 0, 0
	for _, raw := range nodes {
		node, _ := raw.(map[string]any)
		readiness, _ := node["readiness"].(map[string]any)
		switch strVal(readiness, "state") {
		case "ready":
			ready++
		case "blocked":
			blocked++
		}
	}
	rows = append(rows,
		[]string{"TASKS", strconv.Itoa(len(nodes))},
		[]string{"READY", strconv.Itoa(ready)},
		[]string{"BLOCKED", strconv.Itoa(blocked)},
		[]string{"", ""},
		[]string{"TEMP ID", "TITLE", "STATUS", "READINESS", "PREREQS"},
	)
	for _, raw := range nodes {
		node, _ := raw.(map[string]any)
		readiness, _ := node["readiness"].(map[string]any)
		rows = append(rows, []string{
			strVal(node, "temp_id"),
			strVal(node, "title"),
			strVal(node, "status"),
			strVal(readiness, "state"),
			strVal(readiness, "satisfied_prerequisites") + "/" + strVal(readiness, "total_prerequisites"),
		})
	}
	return rows
}
