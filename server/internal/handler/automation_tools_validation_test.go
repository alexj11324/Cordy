package handler

import (
	"net/http/httptest"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func TestAutomationToolsRequireScopedSlackDestination(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database required")
	}
	id := createAutomationAs(t, "", "slack-output-validation")
	foreignWorkspace := dbfx.Workspace(t, "Slack output foreign workspace", "slack-output-foreign")
	foreignInstallation := dbfx.Insert(t, "channel_installation", testutil.Cols{
		"workspace_id": foreignWorkspace, "channel_type": "slack", "config": `{}`, "status": "installed", "installer_user_id": testUserID,
	})
	localInstallation := dbfx.Insert(t, "channel_installation", testutil.Cols{
		"workspace_id": testWorkspaceID, "channel_type": "slack", "config": `{}`, "status": "installed", "installer_user_id": testUserID,
	})
	for _, tc := range []struct {
		name   string
		output map[string]any
		want   int
	}{
		{"missing target", map[string]any{"enabled": true}, 400},
		{"foreign installation", map[string]any{"enabled": true, "installation_id": foreignInstallation, "channel_ids": []string{"C123"}}, 400},
		{"valid target", map[string]any{"enabled": true, "installation_id": localInstallation, "channel_ids": []string{"C123"}}, 200},
	} {
		t.Run(tc.name, func(t *testing.T) {
			w := httptest.NewRecorder()
			r := withURLParam(newRequest("PATCH", "/api/automations/"+id+"?workspace_id="+testWorkspaceID, map[string]any{"tools": map[string]any{"slack_send": tc.output}}), "id", id)
			testHandler.UpdateAutomation(w, r)
			if w.Code != tc.want {
				t.Fatalf("update: %d %s", w.Code, w.Body.String())
			}
		})
	}
	agentID := createHandlerTestAgent(t, "slack-output-create", nil)
	w := httptest.NewRecorder()
	r := newRequest("POST", "/api/automations?workspace_id="+testWorkspaceID, map[string]any{
		"title": "must not create", "executor_id": agentID, "execution_mode": "run_only", "tools": map[string]any{"slack_send": map[string]any{"enabled": true}},
	})
	testHandler.CreateAutomation(w, r)
	if w.Code != 400 {
		t.Fatalf("create without destination: %d %s", w.Code, w.Body.String())
	}
}
