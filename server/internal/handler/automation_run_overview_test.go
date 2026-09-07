package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestWorkspaceAutomationRunOverview(t *testing.T) {
	agentID := createWebhookTestAgent(t, "Overview agent")
	mine := createWebhookTestAutomation(t, agentID, "active", "run_only")
	teammate := createWebhookTestAutomation(t, agentID, "paused", "run_only")
	foreign := createWebhookTestAutomation(t, agentID, "active", "run_only")
	ctx := context.Background()
	exec := func(query string, args ...any) {
		t.Helper()
		if _, err := testPool.Exec(ctx, query, args...); err != nil {
			t.Fatal(err)
		}
	}
	exec(`UPDATE automation SET title = 'Overview mine' WHERE id = $1`, mine)
	exec(`UPDATE automation SET title = 'Overview teammate', created_by_id = gen_random_uuid() WHERE id = $1`, teammate)
	var foreignWorkspace string
	if err := testPool.QueryRow(ctx, `INSERT INTO workspace (name, slug, issue_prefix) VALUES ('Overview foreign', 'overview-' || gen_random_uuid()::text, 'OVF') RETURNING id`).Scan(&foreignWorkspace); err != nil {
		t.Fatal(err)
	}
	exec(`UPDATE automation SET workspace_id = $1 WHERE id = $2`, foreignWorkspace, foreign)
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM automation WHERE id = $1`, foreign)
		testPool.Exec(context.Background(), `DELETE FROM workspace WHERE id = $1`, foreignWorkspace)
	})
	for _, id := range []string{mine, teammate, foreign} {
		id := id
		t.Cleanup(func() { testPool.Exec(context.Background(), `DELETE FROM automation_run WHERE automation_id = $1`, id) })
	}
	// Thirty successes exceed one page. The summary must count all thirty.
	exec(`INSERT INTO automation_run (automation_id, source, status, triggered_at, completed_at)
 SELECT $1, 'manual', 'completed', now() - interval '1 hour', now() FROM generate_series(1, 30)`, mine)
	exec(`INSERT INTO automation_run (automation_id, source, status, triggered_at, completed_at)
 VALUES ($1, 'manual', 'failed', now() - interval '2 days', now()),
 ($1, 'manual', 'failed', now() - interval '8 days', now()),
 ($2, 'manual', 'failed', now() - interval '1 hour', now()),
 ($3, 'manual', 'completed', now() - interval '1 hour', now())`, mine, teammate, foreign)
	exec(`UPDATE automation_run SET failure_reason = 'Provider rejected payload',
 trigger_payload = jsonb_build_object('body', repeat('x', 250 * 1024)),
 result = '{"delivery_error":"kept for run history"}'::jsonb
 WHERE automation_id = $1 AND status = 'failed'`, mine)
	type response struct {
		Runs []struct {
			AutomationRunResponse
			AutomationTitle string `json:"automation_title"`
		} `json:"runs"`
		Summary db.WorkspaceAutomationRunSummaryRow `json:"summary"`
	}
	get := func(query string) response {
		t.Helper()
		w := httptest.NewRecorder()
		testHandler.ListWorkspaceAutomationRuns(w, newRequest(http.MethodGet, "/api/automations/runs?"+query, nil))
		if w.Code != http.StatusOK {
			t.Fatalf("got %d: %s", w.Code, w.Body.String())
		}
		var result response
		if err := json.Unmarshal(w.Body.Bytes(), &result); err != nil {
			t.Fatal(err)
		}
		for _, run := range result.Runs {
			if run.AutomationID == foreign {
				t.Fatal("cross-workspace run leaked")
			}
		}
		return result
	}
	result := get("scope=mine")
	if len(result.Runs) != 25 || result.Summary.Total != 32 || result.Summary.Successful24h != 30 || result.Summary.Successful7d != 30 || result.Summary.Failed24h != 0 || result.Summary.Failed7d != 1 {
		t.Fatalf("incorrect mine page/summary: rows=%d summary=%+v", len(result.Runs), result.Summary)
	}
	second := get("scope=mine&offset=25")
	if len(second.Runs) != 7 {
		t.Fatalf("second page has %d runs", len(second.Runs))
	}
	seen := map[string]bool{}
	for _, run := range result.Runs {
		seen[run.ID] = true
	}
	for _, run := range second.Runs {
		if seen[run.ID] {
			t.Fatal("pagination duplicated a run")
		}
	}
	result = get("scope=team&search=teammate&status=failed")
	if len(result.Runs) != 1 || result.Summary.Total != 1 || result.Summary.Failed24h != 1 || result.Summary.Failed7d != 2 || result.Summary.Successful24h != 30 {
		t.Fatalf("incorrect filtered team data: rows=%d summary=%+v", len(result.Runs), result.Summary)
	}
	// Filtering must search the full history, including beyond the first page.
	result = get("scope=mine&status=failed")
	if len(result.Runs) != 2 || result.Summary.Total != 2 {
		t.Fatalf("status filter truncated to first page: %+v", result)
	}
	result = get("scope=mine&search=Provider+rejected+payload")
	if len(result.Runs) != 2 || result.Summary.Total != 2 || result.Summary.Successful24h != 30 {
		t.Fatalf("failure-reason search lost matching history or changed scope counts: %+v", result)
	}
	for _, run := range result.Runs {
		if run.TriggerPayload != nil {
			t.Fatal("workspace history included the large trigger payload")
		}
		result, ok := run.Result.(map[string]any)
		if !ok || result["delivery_error"] != "kept for run history" {
			t.Fatalf("workspace history lost result details: %+v", run.Result)
		}
	}
	result = get("scope=mine&search=Provider+rejected+payload&status=completed")
	if len(result.Runs) != 0 || result.Summary.Total != 0 {
		t.Fatalf("failure-reason search bypassed the status filter: %+v", result)
	}
	w := httptest.NewRecorder()
	testHandler.ListWorkspaceAutomationRuns(w, newRequest(http.MethodGet, "/api/automations/runs?offset=2147483648", nil))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("overflow offset accepted: %d", w.Code)
	}
}
