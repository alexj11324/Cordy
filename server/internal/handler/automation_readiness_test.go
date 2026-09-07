package handler

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

type handlerAutomationReadinessFailingDBTX struct {
	db.DBTX
	querySubstring string
	err            error
}

func (d handlerAutomationReadinessFailingDBTX) Query(ctx context.Context, query string, args ...any) (pgx.Rows, error) {
	if strings.Contains(query, d.querySubstring) {
		return nil, d.err
	}
	return d.DBTX.Query(ctx, query, args...)
}

// seedValidAutomationScheduleTrigger supplies the minimum persisted trigger
// state required by dispatch tests. Keeping this fixture explicit makes those
// tests exercise the real readiness gate instead of bypassing it.
func seedValidAutomationScheduleTrigger(t *testing.T, automationID string) string {
	t.Helper()
	var triggerID string
	err := testPool.QueryRow(context.Background(), `
		INSERT INTO automation_trigger (
			automation_id, kind, enabled, cron_expression, timezone, next_run_at, provider
		) VALUES ($1, 'schedule', true, '0 * * * *', 'UTC', $2, 'generic')
		RETURNING id`, automationID, time.Now().UTC().Add(time.Hour)).Scan(&triggerID)
	if err != nil {
		t.Fatalf("seed valid automation trigger: %v", err)
	}
	return triggerID
}

func TestGetAutomationReportsTriggerReadiness(t *testing.T) {
	agentID := createWebhookTestAgent(t, "Readiness Agent")
	automationID := createWebhookTestAutomation(t, agentID, "active", "run_only")

	get := func() struct {
		Automation AutomationResponse          `json:"automation"`
		Triggers   []AutomationTriggerResponse `json:"triggers"`
	} {
		t.Helper()
		w := httptest.NewRecorder()
		req := withURLParam(newRequest(http.MethodGet, "/api/automations/"+automationID, nil), "id", automationID)
		testHandler.GetAutomation(w, req)
		if w.Code != http.StatusOK {
			t.Fatalf("GetAutomation: expected 200, got %d: %s", w.Code, w.Body.String())
		}
		var response struct {
			Automation AutomationResponse          `json:"automation"`
			Triggers   []AutomationTriggerResponse `json:"triggers"`
		}
		if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
			t.Fatalf("decode GetAutomation: %v", err)
		}
		return response
	}

	initial := get()
	if initial.Automation.RunReady == nil || *initial.Automation.RunReady {
		t.Fatalf("no-trigger run_ready = %v, want false", initial.Automation.RunReady)
	}
	if len(initial.Automation.RunBlockedReasons) == 0 || !strings.Contains(initial.Automation.RunBlockedReasons[0], "at least one trigger") {
		t.Fatalf("no-trigger blocked reasons = %#v", initial.Automation.RunBlockedReasons)
	}

	create := httptest.NewRecorder()
	req := withURLParam(newRequest(http.MethodPost, "/api/automations/"+automationID+"/triggers", map[string]any{
		"kind":            "schedule",
		"cron_expression": "0 * * * *",
		"timezone":        "UTC",
	}), "id", automationID)
	testHandler.CreateAutomationTrigger(create, req)
	if create.Code != http.StatusCreated {
		t.Fatalf("CreateAutomationTrigger: expected 201, got %d: %s", create.Code, create.Body.String())
	}

	ready := get()
	if ready.Automation.RunReady == nil || !*ready.Automation.RunReady || len(ready.Automation.RunBlockedReasons) != 0 {
		t.Fatalf("configured run readiness = %v reasons=%#v", ready.Automation.RunReady, ready.Automation.RunBlockedReasons)
	}
	if len(ready.Triggers) != 1 || ready.Triggers[0].Ready == nil || !*ready.Triggers[0].Ready || len(ready.Triggers[0].ReadinessReasons) != 0 {
		t.Fatalf("configured trigger readiness = %+v", ready.Triggers)
	}
}

func TestWebhookDeliveryRetriesOnProviderReadinessError(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}
	ctx := context.Background()
	agentID := createWebhookTestAgent(t, "Readiness retry agent")
	automationID := createWebhookTestAutomation(t, agentID, "active", "run_only")

	created := httptest.NewRecorder()
	createReq := withURLParam(newRequest(http.MethodPost, "/api/automations/"+automationID+"/triggers", map[string]any{
		"kind":   "webhook",
		"preset": "slack.channel_created",
	}), "id", automationID)
	testHandler.CreateAutomationTrigger(created, createReq)
	if created.Code != http.StatusCreated {
		t.Fatalf("CreateAutomationTrigger: expected 201, got %d: %s", created.Code, created.Body.String())
	}
	var trigger AutomationTriggerResponse
	if err := json.Unmarshal(created.Body.Bytes(), &trigger); err != nil {
		t.Fatalf("decode trigger: %v", err)
	}

	delivery, _, err := testHandler.persistInboundDeliveryCtx(ctx, persistDeliveryInput{
		WorkspaceID:     parseUUID(testWorkspaceID),
		AutomationID:    parseUUID(automationID),
		TriggerID:       parseUUID(trigger.ID),
		Provider:        "slack",
		Event:           "slack.channel_created",
		SignatureStatus: sigStatusValid,
		ContentType:     "application/json",
		RawBody:         []byte(`{"event":{"type":"channel_created","channel":{"id":"C1"}}}`),
		SelectedHeaders: []byte(`{}`),
	})
	if err != nil {
		t.Fatalf("persist queued delivery: %v", err)
	}

	injected := errors.New("injected Slack provider lookup failure")
	originalHandlerQueries := testHandler.Queries
	originalServiceQueries := testHandler.AutomationService.Queries
	failingQueries := db.New(handlerAutomationReadinessFailingDBTX{
		DBTX:           testPool,
		querySubstring: "ListChannelInstallationsByWorkspace",
		err:            injected,
	})
	testHandler.Queries = failingQueries
	testHandler.AutomationService.Queries = failingQueries
	t.Cleanup(func() {
		testHandler.Queries = originalHandlerQueries
		testHandler.AutomationService.Queries = originalServiceQueries
	})

	worked, processErr := testHandler.WebhookDeliveryWorker.ProcessNext(ctx)
	if processErr != nil || !worked {
		t.Fatalf("provider failure processing: worked=%v err=%v", worked, processErr)
	}
	got, err := testHandler.Queries.GetWebhookDelivery(ctx, delivery.ID)
	if err != nil {
		t.Fatalf("reload delivery: %v", err)
	}
	if got.Status != deliveryStatusQueued || got.DispatchAttempts != 1 {
		t.Fatalf("provider failure delivery = status=%q attempts=%d, want queued/1", got.Status, got.DispatchAttempts)
	}
	if !got.Error.Valid || !strings.Contains(got.Error.String, injected.Error()) {
		t.Fatalf("provider failure delivery error = %v, want injected error", got.Error)
	}
	var runCount int
	if err := testPool.QueryRow(ctx, `SELECT count(*) FROM automation_run WHERE automation_id = $1`, automationID).Scan(&runCount); err != nil {
		t.Fatalf("count terminal runs: %v", err)
	}
	if runCount != 0 {
		t.Fatalf("provider readiness failure created %d terminal runs, want zero", runCount)
	}
}
