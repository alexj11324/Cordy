package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

const dependencyGraphCLIIssueUUID = "11111111-1111-1111-1111-111111111111"

func newDependencyGraphGetTestCmd() *cobra.Command {
	cmd := &cobra.Command{Use: "get"}
	cmd.Flags().String("output", "json", "")
	return cmd
}

func newDependencyGraphApplyTestCmd() *cobra.Command {
	cmd := &cobra.Command{Use: "apply"}
	cmd.Flags().String("idempotency-key", "", "")
	cmd.Flags().String("plan-file", "", "")
	cmd.Flags().Bool("plan-stdin", false, "")
	cmd.Flags().Bool("allow-external-file", false, "")
	cmd.Flags().String("output", "json", "")
	return cmd
}

func TestRunIssueDependencyGraphGetUsesIssueRoute(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.URL.Path != "/api/issues/"+dependencyGraphCLIIssueUUID+"/dependency-graph" {
			t.Fatalf("request = %s %s, want dependency graph GET", r.Method, r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"plan": map[string]any{"id": "plan-1", "status": "active"},
			"nodes": []any{},
			"edges": []any{},
		})
	}))
	defer srv.Close()
	t.Setenv("PATCHBAY_SERVER_URL", srv.URL)
	t.Setenv("PATCHBAY_WORKSPACE_ID", "workspace-1")
	t.Setenv("PATCHBAY_TOKEN", "test-token")

	cmd := newDependencyGraphGetTestCmd()
	out, err := captureStdout(t, func() error {
		return runIssueDependencyGraphGet(cmd, []string{dependencyGraphCLIIssueUUID})
	})
	if err != nil {
		t.Fatalf("runIssueDependencyGraphGet: %v", err)
	}
	if !strings.Contains(out, `"plan-1"`) || !strings.Contains(out, `"active"`) {
		t.Fatalf("JSON output = %s", out)
	}
}

func TestRunIssueDependencyGraphApplyForwardsPlanAndIdempotencyKey(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/api/issues/"+dependencyGraphCLIIssueUUID+"/dependency-graph/apply" {
			t.Fatalf("request = %s %s, want dependency graph apply POST", r.Method, r.URL.Path)
		}
		if got := r.Header.Get("Idempotency-Key"); got != "graph-key" {
			t.Fatalf("Idempotency-Key = %q, want graph-key", got)
		}
		var plan map[string]any
		if err := json.NewDecoder(r.Body).Decode(&plan); err != nil {
			t.Fatalf("decode plan: %v", err)
		}
		if plan["goal"] != "ship" {
			t.Fatalf("plan = %#v, want goal ship", plan)
		}
		_ = json.NewEncoder(w).Encode(map[string]any{
			"plan": map[string]any{"id": "plan-1", "status": "active"},
			"nodes": []any{},
			"edges": []any{},
		})
	}))
	defer srv.Close()
	t.Setenv("PATCHBAY_SERVER_URL", srv.URL)
	t.Setenv("PATCHBAY_WORKSPACE_ID", "workspace-1")
	t.Setenv("PATCHBAY_TOKEN", "test-token")

	cmd := newDependencyGraphApplyTestCmd()
	_ = cmd.Flags().Set("idempotency-key", " graph-key ")
	_ = cmd.Flags().Set("plan-stdin", "true")
	cmd.SetIn(strings.NewReader(`{"goal":"ship","tasks":[],"edges":[]}`))
	out, err := captureStdout(t, func() error {
		return runIssueDependencyGraphApply(cmd, []string{dependencyGraphCLIIssueUUID})
	})
	if err != nil {
		t.Fatalf("runIssueDependencyGraphApply: %v", err)
	}
	if !strings.Contains(out, `"plan-1"`) {
		t.Fatalf("JSON output = %s", out)
	}
}

func TestReadDependencyGraphPlanRequiresExactlyOneSource(t *testing.T) {
	cmd := newDependencyGraphApplyTestCmd()
	if _, err := readDependencyGraphPlan(cmd); err == nil {
		t.Fatal("readDependencyGraphPlan accepted no plan source")
	}
	_ = cmd.Flags().Set("plan-file", "plan.json")
	_ = cmd.Flags().Set("plan-stdin", "true")
	if _, err := readDependencyGraphPlan(cmd); err == nil || !strings.Contains(err.Error(), "mutually exclusive") {
		t.Fatalf("readDependencyGraphPlan error = %v, want mutually exclusive", err)
	}
}

func TestDependencyGraphTableIncludesReadiness(t *testing.T) {
	graph := map[string]any{
		"plan": map[string]any{"id": "plan-1", "goal": "Ship graph", "status": "active"},
		"nodes": []any{
			map[string]any{
				"temp_id": "root", "title": "Root", "status": "todo",
				"readiness": map[string]any{"state": "ready", "satisfied_prerequisites": 0, "total_prerequisites": 0},
			},
			map[string]any{
				"temp_id": "child", "title": "Child", "status": "blocked",
				"readiness": map[string]any{"state": "blocked", "satisfied_prerequisites": 0, "total_prerequisites": 1},
			},
		},
	}
	rows := dependencyGraphTableRows(graph)
	encoded := ""
	for _, row := range rows {
		encoded += strings.Join(row, " ") + "\n"
	}
	for _, want := range []string{"PLAN plan-1", "READY 1", "BLOCKED 1", "root Root todo ready 0/0", "child Child blocked blocked 0/1"} {
		if !strings.Contains(encoded, want) {
			t.Fatalf("table rows missing %q:\n%s", want, encoded)
		}
	}
}
