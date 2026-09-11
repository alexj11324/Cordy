package handler

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/redis/go-redis/v9"

	"github.com/orvilo-ai/orvilo/server/internal/integrations/weixin"
	"github.com/orvilo-ai/orvilo/server/internal/testutil"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type weixinQRTransport func(*http.Request) (*http.Response, error)

func (f weixinQRTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestBeginWeixinInstallReturnsScanURLInsteadOfPollingToken(t *testing.T) {
	withManagedMessaging(t)
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
	var optionalRedis *redis.Client
	weixin.ConfigureSessionStore(optionalRedis)
	t.Cleanup(func() { weixin.ConfigureSessionStore(nil) })
	previous := http.DefaultTransport
	http.DefaultTransport = weixinQRTransport(func(r *http.Request) (*http.Response, error) {
		if r.URL.Path != "/ilink/bot/get_bot_qrcode" {
			t.Fatalf("unexpected provider request: %s", r.URL.Path)
		}
		return &http.Response{StatusCode: http.StatusOK, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(
			`{"qrcode":"opaque-polling-token","qrcode_img_content":"https://weixin.qq.com/x/scan-this-code"}`,
		))}, nil
	})
	t.Cleanup(func() { http.DefaultTransport = previous })
	req := withURLParam(newRequest(http.MethodPost, "/api/workspaces/"+testWorkspaceID+"/weixin/install/begin", nil), "id", testWorkspaceID)
	w := httptest.NewRecorder()
	testHandler.BeginWeixinInstall(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("begin status = %d, body = %s", w.Code, w.Body.String())
	}
	var response struct {
		SessionID string `json:"session_id"`
		ScanURL   string `json:"qr_code_url"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = weixin.DefaultInstallSessionStore().Delete(t.Context(), response.SessionID) })
	if response.ScanURL != "https://weixin.qq.com/x/scan-this-code" {
		t.Fatalf("scan URL = %q; the opaque polling token must not be rendered as the QR", response.ScanURL)
	}
}

func TestListWeixinInstallationsNotConfiguredReturnsEmpty(t *testing.T) {
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "")
	h := &Handler{}
	req := httptest.NewRequest(http.MethodGet, "/api/workspaces/x/weixin/installations", nil)
	w := httptest.NewRecorder()

	h.ListWeixinInstallations(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", w.Code, w.Body.String())
	}
	var response struct {
		Installations    []any `json:"installations"`
		Configured       bool  `json:"configured"`
		InstallSupported bool  `json:"install_supported"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	if response.Configured || response.InstallSupported || len(response.Installations) != 0 {
		t.Fatalf("unexpected unconfigured response: %+v", response)
	}
}

func TestListWeixinInstallationsHonorsDeploymentMode(t *testing.T) {
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
	installationID := dbfx.Insert(t, "channel_installation", testutil.Cols{
		"workspace_id":      testWorkspaceID,
		"channel_type":      "weixin",
		"config":            []byte(`{"app_id":"weixin-listing-mode","ilink_user_id":"weixin-listing-user"}`),
		"installer_user_id": testUserID,
		"status":            "installed",
	})
	for _, test := range []struct {
		mode, appURL string
		enabled      bool
	}{
		{"disabled", "https://orvilo.aspectlylabs.com", false},
		{"server_configured", "https://app.example.test", true},
		{"managed", "https://orvilo.aspectlylabs.com", true},
	} {
		t.Run(test.mode, func(t *testing.T) {
			t.Setenv("ORVILO_APP_URL", test.appURL)
			t.Setenv("ORVILO_MESSAGING_MODE", test.mode)
			req := withURLParam(newRequest(http.MethodGet, "/api/workspaces/"+testWorkspaceID+"/weixin/installations", nil), "id", testWorkspaceID)
			w := httptest.NewRecorder()
			testHandler.ListWeixinInstallations(w, req)
			if w.Code != http.StatusOK {
				t.Fatalf("list status = %d, body = %s", w.Code, w.Body.String())
			}
			var response struct {
				Installations    []WeixinInstallationResponse `json:"installations"`
				Configured       bool                         `json:"configured"`
				InstallSupported bool                         `json:"install_supported"`
			}
			if err := json.Unmarshal(w.Body.Bytes(), &response); err != nil {
				t.Fatal(err)
			}
			if response.Configured != test.enabled || response.InstallSupported != test.enabled {
				t.Fatalf("mode %s returned configured=%v, install_supported=%v", test.mode, response.Configured, response.InstallSupported)
			}
			if !test.enabled && len(response.Installations) != 0 {
				t.Fatal("disabled deployment exposed installation rows")
			}
			if test.enabled && (len(response.Installations) != 1 || response.Installations[0].ID != installationID) {
				t.Fatalf("enabled deployment lost its readable installation: %+v", response.Installations)
			}
		})
	}
}

func TestWeixinMutationHandlersRejectUnconfiguredDeployment(t *testing.T) {
	t.Setenv("ORVILO_WEIXIN_SECRET_KEY", "")
	tests := []struct {
		name string
		verb string
		path string
		body string
		run  func(*Handler, http.ResponseWriter, *http.Request)
	}{
		{name: "begin", verb: http.MethodPost, path: "/api/workspaces/x/weixin/install/begin?agent_id=y", run: (*Handler).BeginWeixinInstall},
		{name: "status", verb: http.MethodGet, path: "/api/workspaces/x/weixin/install/y/status", run: (*Handler).GetWeixinInstallStatus},
		{name: "revoke", verb: http.MethodDelete, path: "/api/workspaces/x/weixin/installations/y", run: (*Handler).RevokeWeixinInstallation},
		{name: "redeem", verb: http.MethodPost, path: "/api/weixin/binding/redeem", body: `{"token":"placeholder"}`, run: (*Handler).RedeemWeixinBindingToken},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			h := &Handler{}
			req := httptest.NewRequest(tt.verb, tt.path, strings.NewReader(tt.body))
			w := httptest.NewRecorder()
			tt.run(h, w, req)
			if w.Code != http.StatusServiceUnavailable {
				t.Fatalf("expected 503, got %d body=%s", w.Code, w.Body.String())
			}
		})
	}
}

func TestWeixinInstallationResponseNeverExposesStoredCredential(t *testing.T) {
	now := time.Date(2026, 8, 14, 1, 0, 0, 0, time.UTC)
	row := db.ChannelInstallation{
		ID:              testWeixinUUID(t, "11111111-1111-1111-1111-111111111111"),
		WorkspaceID:     testWeixinUUID(t, "22222222-2222-2222-2222-222222222222"),
		AgentID:         testWeixinUUID(t, "33333333-3333-3333-3333-333333333333"),
		InstallerUserID: testWeixinUUID(t, "44444444-4444-4444-4444-444444444444"),
		Status:          "installed",
		Config:          json.RawMessage(`{"app_id":"bot-id","ilink_user_id":"wx-user","bot_token_encrypted":"ciphertext-sentinel"}`),
		InstalledAt:     pgtype.Timestamptz{Time: now, Valid: true},
		CreatedAt:       pgtype.Timestamptz{Time: now, Valid: true},
		UpdatedAt:       pgtype.Timestamptz{Time: now, Valid: true},
	}

	response := weixinInstallationToResponse(row)
	payload, err := json.Marshal(response)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(payload), "ciphertext-sentinel") || strings.Contains(string(payload), "bot_token") {
		t.Fatalf("installation response exposed stored credential: %s", payload)
	}
	if response.BotID != "bot-id" || response.ILinkUserID != "wx-user" {
		t.Fatalf("public provider identity = %#v", response)
	}
}

func TestWeixinCreatedEventOnlyPublishesForFreshSuccess(t *testing.T) {
	installationID := testWeixinUUID(t, "55555555-5555-5555-5555-555555555555")
	cases := []struct {
		name   string
		result weixin.StatusResult
		want   bool
	}{
		{
			name: "fresh success",
			result: weixin.StatusResult{
				Status:         weixin.InstallStatusSuccess,
				InstallationID: installationID,
				Created:        true,
			},
			want: true,
		},
		{
			name: "repeated success",
			result: weixin.StatusResult{
				Status:         weixin.InstallStatusSuccess,
				InstallationID: installationID,
			},
			want: false,
		},
		{
			name: "pending",
			result: weixin.StatusResult{
				Status:  weixin.InstallStatusPending,
				Created: true,
			},
			want: false,
		},
	}
	for _, tt := range cases {
		t.Run(tt.name, func(t *testing.T) {
			if got := shouldPublishWeixinInstallationCreated(tt.result); got != tt.want {
				t.Fatalf("shouldPublishWeixinInstallationCreated() = %v, want %v", got, tt.want)
			}
		})
	}
}

func testWeixinUUID(t *testing.T, value string) pgtype.UUID {
	t.Helper()
	var id pgtype.UUID
	if err := id.Scan(value); err != nil {
		t.Fatalf("parse UUID %q: %v", value, err)
	}
	return id
}
