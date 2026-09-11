package handler

import (
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/orvilo-ai/orvilo/server/internal/testutil"
)

func withManagedMessaging(t *testing.T) {
	t.Helper()
	t.Setenv("ORVILO_APP_URL", "https://orvilo.aspectlylabs.com")
	t.Setenv("ORVILO_MESSAGING_MODE", "managed")
}

func TestMessagingModeGatesEveryInstallationWrite(t *testing.T) {
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
		{"finish slack managed", h.ManagedSlackOAuthCallback},
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
	for _, mode := range []struct {
		name, appURL, requested, code string
		status                        int
	}{
		{"self hosted", "https://app.example.test", "server_configured", "server_managed_integration", http.StatusForbidden},
		{"disabled", "https://orvilo.aspectlylabs.com", "disabled", "messaging_disabled", http.StatusServiceUnavailable},
	} {
		t.Run(mode.name, func(t *testing.T) {
			t.Setenv("ORVILO_APP_URL", mode.appURL)
			t.Setenv("ORVILO_MESSAGING_MODE", mode.requested)
			for _, test := range tests {
				t.Run(test.name, func(t *testing.T) {
					body := testutil.Call(t, test.run, httptest.NewRequest(http.MethodPost, "/", nil)).Want(mode.status).Map()
					if body["code"] != mode.code {
						t.Fatalf("code = %q, want %q", body["code"], mode.code)
					}
				})
			}
		})
	}
}
