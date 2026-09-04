package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestServerConfiguredMessagingRejectsEveryInstallationWrite(t *testing.T) {
	t.Setenv("PATCHBAY_APP_URL", "https://app.example.test")
	t.Setenv("PATCHBAY_MESSAGING_MODE", "server_configured")
	h := &Handler{}
	tests := []struct {
		name string
		run  func(http.ResponseWriter, *http.Request)
	}{
		{"begin lark", h.BeginLarkInstall},
		{"poll lark finalize", h.GetLarkInstallStatus},
		{"revoke lark", h.RevokeLarkInstallation},
		{"install slack byo", h.RegisterSlackBYO},
		{"begin slack managed", h.BeginManagedSlackInstall},
		{"revoke slack", h.RevokeSlackInstallation},
		{"install dingtalk", h.RegisterDingTalkBYO},
		{"revoke dingtalk", h.RevokeDingTalkInstallation},
		{"install wecom", h.RegisterWecomBYO},
		{"revoke wecom", h.RevokeWecomInstallation},
		{"install telegram", h.RegisterTelegramBot},
		{"revoke telegram", h.RevokeTelegramInstallation},
		{"begin weixin", h.BeginWeixinInstall},
		{"poll weixin finalize", h.GetWeixinInstallStatus},
		{"revoke weixin", h.RevokeWeixinInstallation},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			recorder := httptest.NewRecorder()
			test.run(recorder, httptest.NewRequest(http.MethodPost, "/", nil))
			if recorder.Code != http.StatusForbidden {
				t.Fatalf("status = %d, want 403; body=%s", recorder.Code, recorder.Body.String())
			}
			var body map[string]string
			if err := json.Unmarshal(recorder.Body.Bytes(), &body); err != nil {
				t.Fatal(err)
			}
			if body["code"] != "server_managed_integration" {
				t.Fatalf("code = %q", body["code"])
			}
		})
	}
}
