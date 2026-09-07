package handler

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func TestNativeSlackAuthenticatedSenderRequiresExistingBinding(t *testing.T) {
	agentID := createWebhookTestAgent(t, "Authenticated Slack sender agent")
	automationID := createWebhookTestAutomation(t, agentID, "active", "run_only")
	installationID := "44444444-4444-4444-4444-444444444444"
	recorder := httptest.NewRecorder()
	request := withURLParam(newRequest(http.MethodPost, "/api/automations/"+automationID+"/triggers", map[string]any{
		"kind": "webhook", "preset": "slack.message",
		"config": map[string]any{
			"installation_id": installationID,
			"channel":         "C1", "sender_scope": "authenticated",
			"ignore_thread_replies": true, "completion_reaction": "none",
		},
	}), "id", automationID)
	testHandler.CreateAutomationTrigger(recorder, request)
	if recorder.Code != http.StatusCreated {
		t.Fatalf("create trigger = %d %s", recorder.Code, recorder.Body.String())
	}

	inst := db.ChannelInstallation{ID: parseUUID(installationID), WorkspaceID: parseUUID(testWorkspaceID)}
	testHandler.HandleSlackNativeAutomation(context.Background(), inst,
		[]byte(`{"event_id":"EvUnbound","event":{"type":"message","user":"U1","channel":"C1","text":"hello","ts":"1.0"}}`))
	if got := len(listDeliveries(t, automationID)); got != 0 {
		t.Fatalf("unbound sender created %d deliveries", got)
	}

	dbfx.Insert(t, "channel_user_binding", testutil.Cols{
		"workspace_id": testWorkspaceID, "orvilo_user_id": testUserID,
		"installation_id": installationID, "channel_type": "slack", "channel_user_id": "U1",
		"config": []byte(`{}`),
	})
	testHandler.HandleSlackNativeAutomation(context.Background(), inst,
		[]byte(`{"event_id":"EvBound","event":{"type":"message","user":"U1","channel":"C1","text":"hello","ts":"2.0"}}`))
	if got := len(listDeliveries(t, automationID)); got != 1 {
		t.Fatalf("bound sender created %d deliveries, want 1", got)
	}
}
