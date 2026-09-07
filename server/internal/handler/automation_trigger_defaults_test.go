package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestCreateSlackMessageTriggerSeedsObservableDefaults(t *testing.T) {
	agentID := createWebhookTestAgent(t, "Slack defaults agent")
	automationID := createWebhookTestAutomation(t, agentID, "active", "run_only")
	recorder := httptest.NewRecorder()
	request := withURLParam(newRequest(http.MethodPost, "/api/automations/"+automationID+"/triggers", map[string]any{
		"kind": "webhook", "preset": "slack.message",
	}), "id", automationID)

	testHandler.CreateAutomationTrigger(recorder, request)
	if recorder.Code != http.StatusCreated {
		t.Fatalf("create trigger = %d %s", recorder.Code, recorder.Body.String())
	}
	var response AutomationTriggerResponse
	if err := json.Unmarshal(recorder.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	config, ok := response.Config.(map[string]any)
	if !ok {
		t.Fatalf("config = %#v", response.Config)
	}
	if config["sender_scope"] != "anyone" || config["ignore_thread_replies"] != true || config["completion_reaction"] != "white_check_mark" {
		t.Fatalf("Slack observable defaults = %#v", config)
	}
}
