package weixin

import (
	"encoding/base64"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/util"
)

func TestInstallationStatusFencesWorkspaceAndInitiatorBeforeProviderCall(t *testing.T) {
	var calls int
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.URL.Path != "/ilink/bot/get_qrcode_status" {
			t.Errorf("path = %q", r.URL.Path)
		}
		_, _ = io.WriteString(w, `{"status":"wait"}`)
	}))
	defer server.Close()

	workspaceID := testInstallUUID(t, "11111111-1111-1111-1111-111111111111")
	otherWorkspaceID := testInstallUUID(t, "22222222-2222-2222-2222-222222222222")
	actorID := testInstallUUID(t, "33333333-3333-3333-3333-333333333333")
	store := NewMemorySessionStore()
	session := InstallSession{
		ID: "session-1", WorkspaceID: util.UUIDToString(workspaceID), InitiatorID: util.UUIDToString(actorID),
		QRCode: "qr", BaseURL: DefaultBaseURL, ExpiresAt: time.Now().Add(time.Minute), Status: InstallStatusPending,
	}
	if err := store.Put(t.Context(), session); err != nil {
		t.Fatal(err)
	}
	service := &InstallationService{
		sessions: store, httpClient: server.Client(), now: time.Now,
		newClient: func(_, token string, client *http.Client) *Client {
			return NewClient(server.URL, token, client)
		},
	}
	if _, err := service.Status(t.Context(), session.ID, otherWorkspaceID, actorID, ""); !errors.Is(err, ErrInstallSessionForbidden) {
		t.Fatalf("cross-workspace status error = %v", err)
	}
	if _, err := service.Status(t.Context(), session.ID, workspaceID, testInstallUUID(t, "44444444-4444-4444-4444-444444444444"), ""); !errors.Is(err, ErrInstallSessionForbidden) {
		t.Fatalf("cross-actor status error = %v", err)
	}
	if calls != 0 {
		t.Fatalf("provider was called before session fence: %d", calls)
	}
	result, err := service.Status(t.Context(), session.ID, workspaceID, actorID, "")
	if err != nil || result.Status != InstallStatusPending {
		t.Fatalf("valid status = %#v, %v", result, err)
	}
	if calls != 1 {
		t.Fatalf("provider calls = %d, want 1", calls)
	}
}

func TestInstallationStatusPersistsValidatedRedirectHost(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"status":"scanned_but_redirect","redirect_host":"region.weixin.qq.com/path"}`)
	}))
	defer server.Close()
	workspaceID := testInstallUUID(t, "55555555-5555-5555-5555-555555555555")
	actorID := testInstallUUID(t, "66666666-6666-6666-6666-666666666666")
	store := NewMemorySessionStore()
	session := InstallSession{ID: "redirect-session", WorkspaceID: util.UUIDToString(workspaceID), InitiatorID: util.UUIDToString(actorID), QRCode: "qr", ExpiresAt: time.Now().Add(time.Minute), BaseURL: DefaultBaseURL}
	if err := store.Put(t.Context(), session); err != nil {
		t.Fatal(err)
	}
	service := &InstallationService{
		sessions: store, httpClient: server.Client(), now: time.Now,
		newClient: func(_, token string, client *http.Client) *Client { return NewClient(server.URL, token, client) },
	}
	result, err := service.Status(t.Context(), session.ID, workspaceID, actorID, "")
	if err != nil || result.Status != InstallStatusScanned {
		t.Fatalf("redirect status = %#v, %v", result, err)
	}
	updated, err := store.Get(t.Context(), session.ID)
	if err != nil || updated.BaseURL != "https://region.weixin.qq.com" {
		t.Fatalf("stored redirect = %#v, %v", updated, err)
	}
}

func TestValidateInstallConfigPinsProviderIdentityAndSecret(t *testing.T) {
	sealed := base64.StdEncoding.EncodeToString([]byte("sealed"))
	config, err := encodeInstallConfig("bot-id", "wx-user", DefaultBaseURL, sealed)
	if err != nil {
		t.Fatal(err)
	}
	if err := validateInstallConfig("bot-id", "wx-user", config); err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct {
		name  string
		botID string
		user  string
		cfg   []byte
	}{
		{name: "bot mismatch", botID: "other-bot", user: "wx-user", cfg: config},
		{name: "user mismatch", botID: "bot-id", user: "other-user", cfg: config},
		{name: "malformed config", botID: "bot-id", user: "wx-user", cfg: []byte("not-json")},
		{name: "missing secret", botID: "bot-id", user: "wx-user", cfg: []byte(`{"app_id":"bot-id","ilink_user_id":"wx-user"}`)},
	} {
		t.Run(test.name, func(t *testing.T) {
			if err := validateInstallConfig(test.botID, test.user, test.cfg); err == nil {
				t.Fatal("invalid installation config was accepted")
			}
		})
	}
}

func testInstallUUID(t *testing.T, value string) pgtype.UUID {
	t.Helper()
	var id pgtype.UUID
	if err := id.Scan(value); err != nil {
		t.Fatalf("parse UUID %q: %v", value, err)
	}
	return id
}
