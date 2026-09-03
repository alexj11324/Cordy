package slack

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestManagedOAuthRetainsEncryptedRotatingCredentials(t *testing.T) {
	now := time.Date(2026, time.September, 3, 12, 0, 0, 0, time.UTC)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"ok":true,"access_token":"access-fixture","refresh_token":"refresh-fixture","expires_in":43200,"app_id":"APP1","team":{"id":"TEAM1"},"bot_user_id":"BOT1"}`))
	}))
	defer server.Close()
	oauth := &ManagedOAuthService{
		httpClient: server.Client(), tokenURL: server.URL, clientID: "client-fixture", clientSecret: "secret-fixture",
		now: func() time.Time { return now },
	}
	access, err := oauth.ExchangeCode(context.Background(), "code-fixture", "https://app.example.test/callback")
	if err != nil {
		t.Fatal(err)
	}
	q := &fakeInstallQueries{}
	install := newTestInstallService(t, q)
	if _, err := install.RegisterManaged(context.Background(), RegisterManagedParams{
		WorkspaceID: testUUID(1), InstallerID: testUUID(2), Access: access,
	}, nil); err != nil {
		t.Fatal(err)
	}
	var stored struct {
		RefreshToken string     `json:"refresh_token_encrypted"`
		ExpiresAt    *time.Time `json:"token_expires_at"`
	}
	if err := json.Unmarshal(q.byAppIDParams.Config, &stored); err != nil {
		t.Fatal(err)
	}
	if stored.RefreshToken == "" {
		t.Fatal("managed OAuth discarded the refresh token before persistence")
	}
	if strings.Contains(string(q.byAppIDParams.Config), "refresh-fixture") || strings.Contains(string(q.byAppIDParams.Config), "access-fixture") {
		t.Fatal("managed credentials must not be stored as plaintext")
	}
	plain, err := decryptToken(stored.RefreshToken, install.box.Open)
	if err != nil || plain != "refresh-fixture" {
		t.Fatal("stored refresh credential cannot be decrypted with the installation key")
	}
	if stored.ExpiresAt == nil || !stored.ExpiresAt.Equal(now.Add(12*time.Hour)) {
		t.Fatalf("access-token expiry = %v, want twelve hours after exchange", stored.ExpiresAt)
	}
}
